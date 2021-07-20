use crate::*;

use crate::block::{Block, BlockPtr, ItemContent, ID};
use crate::block_store::StateVector;
use crate::event::UpdateEvent;
use crate::id_set::{DeleteSet, IdSet};
use crate::store::Store;
use crate::types::{Map, Text, TypePtr, XorHasher};
use crate::update::Update;
use std::cell::RefMut;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use std::ops::Range;
use updates::encoder::*;

pub struct Transaction<'a> {
    /// Store containing the state of the document.
    pub store: RefMut<'a, Store>,
    /// State vector of a current transaction.
    pub timestamp: StateVector,
    /// ID's of the blocks to be merged.
    pub merge_blocks: Vec<ID>,
    /// Describes the set of deleted items by ids.
    pub delete_set: DeleteSet,
    /// All types that were directly modified (property added or child inserted/deleted).
    /// New types are not included in this Set.
    changed: HashMap<TypePtr, HashSet<Option<String>>, BuildHasherDefault<XorHasher>>,
}

impl<'a> Transaction<'a> {
    pub fn new(store: RefMut<'a, Store>) -> Transaction {
        let begin_timestamp = store.blocks.get_state_vector();
        Transaction {
            store,
            timestamp: begin_timestamp,
            merge_blocks: Vec::new(),
            delete_set: DeleteSet::new(),
            changed: HashMap::with_hasher(BuildHasherDefault::default()),
        }
    }

    pub fn get_text(&mut self, name: &str) -> Text {
        let ptr = self.store.create_type_ptr(name);
        Text::from(ptr)
    }

    pub fn get_map(&mut self, name: &str) -> Map {
        let ptr = self.store.create_type_ptr(name);
        Map::from(ptr)
    }

    /// Encodes the document state to a binary format.
    ///
    /// Document updates are idempotent and commutative. Caveats:
    /// * It doesn't matter in which order document updates are applied.
    /// * As long as all clients receive the same document updates, all clients
    ///   end up with the same content.
    /// * Even if an update contains known information, the unknown information
    ///   is extracted and integrated into the document structure.
    pub fn encode_update(&self) -> Vec<u8> {
        let mut update_encoder = updates::encoder::EncoderV1::new();
        self.store.encode_diff(&self.timestamp, &mut update_encoder);
        update_encoder.to_vec()
    }

    pub fn iterate_structs<F>(&mut self, client: &u64, range: &Range<u32>, f: &F)
    where
        F: Fn(&Block) -> (),
    {
        let clock_start = range.start;
        let clock_end = range.end;

        if clock_start == clock_end {
            return;
        }

        if let Some(mut index) = self.find_index_clean_start(client, clock_start) {
            let mut blocks = self.store.blocks.get(client).unwrap();
            let mut block = &blocks[index];

            while index < blocks.len() && block.id().clock < clock_end {
                if clock_end < block.clock_end() {
                    self.find_index_clean_start(client, clock_start);
                    blocks = self.store.blocks.get(client).unwrap();
                    block = &blocks[index];
                }

                f(block);
                index += 1;

                block = &blocks[index];
            }
        }
    }

    pub fn find_index_clean_start(&mut self, client: &u64, clock: u32) -> Option<usize> {
        let blocks = self.store.blocks.get_mut(client)?;
        let index = blocks.find_pivot(clock)?;
        let block = &mut blocks[index];
        if let Some(item) = block.as_item_mut() {
            if item.id.clock < clock {
                // if we run over the clock, we need to the split item
                let id = ID::new(*client, clock - item.id.clock);
                self.store
                    .blocks
                    .split_block(&BlockPtr::new(id, index as u32));
                return Some(index + 1);
            }
        }

        Some(index)
    }

    pub fn apply_ranges<F>(&mut self, set: &IdSet, f: &F)
    where
        F: Fn(&Block) -> (),
    {
        // equivalent of JS: Y.iterateDeletedStructs
        for (client, ranges) in set.iter() {
            if self.store.blocks.contains_client(client) {
                for range in ranges.iter() {
                    self.iterate_structs(client, range, f);
                }
            }
        }
    }

    /// Applies given `id_set` onto current transaction to run multi-range deletion.
    /// Returns a remaining of original ID set, that couldn't be applied.
    pub fn apply_delete(&mut self, ds: &DeleteSet) -> Option<DeleteSet> {
        let mut unapplied = DeleteSet::new();
        for (client, ranges) in ds.iter() {
            let mut blocks = self.store.blocks.get_mut(client).unwrap();
            let state = blocks.get_state();

            for range in ranges.iter() {
                let clock = range.start;
                let clock_end = range.end;

                if clock < state {
                    if state < clock_end {
                        unapplied.insert(ID::new(*client, clock), clock_end - state);
                    }
                    // We can ignore the case of GC and Delete structs, because we are going to skip them
                    if let Some(mut index) = blocks.find_pivot(clock) {
                        // We can ignore the case of GC and Delete structs, because we are going to skip them
                        if let Some(item) = blocks[index].as_item_mut() {
                            // split the first item if necessary
                            if !item.deleted && item.id.clock < clock {
                                let split_ptr = BlockPtr::new(
                                    ID::new(*client, clock - item.id.clock),
                                    index as u32,
                                );
                                let (_, right) = self.store.blocks.split_block(&split_ptr);
                                blocks = self.store.blocks.get_mut(client).unwrap();
                                index += 1;
                                self.merge_blocks.push(right.unwrap().id);
                            }

                            while index < blocks.len() {
                                let block = &mut blocks[index];
                                if let Some(item) = block.as_item_mut() {
                                    if item.id.clock < clock_end {
                                        if !item.deleted {
                                            let delete_ptr =
                                                BlockPtr::new(item.id.clone(), index as u32);
                                            if item.id.clock + item.content.len() > clock_end {
                                                let diff = clock_end - item.id.clock;
                                                let mut split_ptr = delete_ptr.clone();
                                                split_ptr.id.clock += diff;
                                                let (_, right) =
                                                    self.store.blocks.split_block(&split_ptr);
                                                if let Some(right) = right {
                                                    self.merge_blocks.push(right.id);
                                                    index += 1;
                                                }
                                            }
                                            self.delete(&delete_ptr);
                                            blocks = self.store.blocks.get_mut(client).unwrap();
                                            // just to make the borrow checker happy
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                index += 1;
                            }
                        }
                    }
                } else {
                    unapplied.insert(ID::new(*client, clock), clock_end - clock);
                }
            }
        }

        if unapplied.is_empty() {
            None
        } else {
            Some(unapplied)
        }
    }

    fn delete(&mut self, ptr: &BlockPtr) {
        if let Some(item) = self.store.blocks.get_item_mut(&ptr) {
            if !item.deleted {
                //TODO:
                // if let Some(parent) = self.store.get_type(&item.parent) {
                //     // adjust the length of parent
                //     if (this.countable && this.parentSub === null) {
                //         parent._length -= this.length
                //     }
                // }
                item.mark_as_deleted();
                self.delete_set.insert(item.id.clone(), item.len());
                // addChangedTypeToTransaction(transaction, item.type, item.parentSub)
                if item.id.clock < self.timestamp.get(&item.id.client) {
                    let set = self.changed.entry(item.parent.clone()).or_default();
                    set.insert(item.parent_sub.clone());
                }
                // item.content.delete(transaction)
                match &mut item.content {
                    ItemContent::Doc(_s, _value) => {
                        todo!()
                    }
                    ItemContent::Type(_inner) => {
                        todo!()
                    }
                    _ => {} // do nothing
                }
            }
        }
    }

    pub fn apply_update(&mut self, mut update: Update, mut ds: DeleteSet) {
        if self.store.update_events.has_subscribers() {
            let event = UpdateEvent::new(update, ds);
            self.store.update_events.publish(&event);
            update = event.update;
            ds = event.delete_set;
        }
        let remaining = update.integrate(self);

        let mut retry = false;
        if let Some(mut pending) = self.store.pending.take() {
            // check if we can apply something
            for (client, &clock) in pending.missing.iter() {
                if clock < self.store.blocks.get_state(client) {
                    retry = true;
                    break;
                }
            }

            if let Some(remaining) = remaining {
                // merge restStructs into store.pending
                for (&client, &clock) in remaining.missing.iter() {
                    pending.missing.set_min(client, clock);
                }
                pending.update.merge(remaining.update);
                self.store.pending = Some(pending);
            }
        } else {
            self.store.pending = remaining;
        }

        let ds = self.apply_delete(&ds);
        if let Some(pending) = self.store.pending_ds.take() {
            let ds2 = self.apply_delete(&pending);
            let ds = match (ds, ds2) {
                (Some(mut a), Some(b)) => {
                    a.merge(b);
                    Some(a)
                }
                (Some(x), _) | (_, Some(x)) => Some(x),
                _ => None,
            };
            self.store.pending_ds = ds;
        } else {
            self.store.pending_ds = ds;
        }

        if retry {
            if let Some(pending) = self.store.pending.take() {
                let ds = self.store.pending_ds.take().unwrap_or_default();
                self.apply_update(pending.update, ds);
            }
        }
    }

    pub fn create_item(
        &mut self,
        pos: &block::ItemPosition,
        content: block::ItemContent,
        parent_sub: Option<String>,
    ) {
        let left = pos.left;
        let right = pos.right;
        let origin = if let Some(ptr) = pos.left.as_ref() {
            if let Some(item) = self.store.blocks.get_item(ptr) {
                Some(item.last_id())
            } else {
                None
            }
        } else {
            None
        };
        let client_id = self.store.client_id;
        let id = block::ID {
            client: client_id,
            clock: self.store.get_local_state(),
        };
        let pivot = self
            .store
            .blocks
            .get_client_blocks_mut(client_id)
            .integrated_len() as u32;

        let mut item = block::Item {
            id,
            content,
            left,
            right,
            origin,
            right_origin: right.map(|r| r.id),
            parent: pos.parent.clone(),
            deleted: false,
            parent_sub,
        };
        item.integrate(self, pivot, 0);
        let local_block_list = self.store.blocks.get_client_blocks_mut(client_id);
        local_block_list.push(block::Block::Item(item));
    }
}
