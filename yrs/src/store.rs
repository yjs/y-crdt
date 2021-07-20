use crate::block_store::{BlockStore, StateVector};
use crate::event::{EventHandler, UpdateEvent};
use crate::id_set::DeleteSet;
use crate::update::PendingUpdate;
use crate::updates::encoder::{Encode, Encoder};
use crate::{block, types};
use std::collections::HashMap;

pub struct Store {
    pub client_id: u64,
    type_refs: HashMap<String, u32>,
    types: Vec<(types::Inner, String)>,
    pub blocks: BlockStore,
    pub pending: Option<PendingUpdate>,
    pub pending_ds: Option<DeleteSet>,
    pub(crate) update_events: EventHandler<UpdateEvent>,
}

impl Store {
    pub fn new(client_id: u64) -> Self {
        Store {
            client_id,
            type_refs: Default::default(),
            types: Default::default(),
            blocks: BlockStore::new(),
            pending: None,
            pending_ds: None,
            update_events: EventHandler::new(),
        }
    }

    pub fn get_local_state(&self) -> u32 {
        self.blocks.get_state(&self.client_id)
    }

    pub fn get_type(&self, ptr: &types::TypePtr) -> Option<&types::Inner> {
        match ptr {
            types::TypePtr::NamedRef(name_ref) => self.types.get(*name_ref as usize).map(|t| &t.0),
            types::TypePtr::Id(id) => {
                // @todo the item might not exist
                let item = self.blocks.get_item(id)?;
                if let block::ItemContent::Type(t) = &item.content {
                    Some(t)
                } else {
                    None
                }
            }
            types::TypePtr::Named(name) => self
                .get_type_ref(name)
                .map(|tref| &self.types[tref as usize].0),
        }
    }

    pub fn get_type_mut(&mut self, ptr: &types::TypePtr) -> Option<&mut types::Inner> {
        match ptr {
            types::TypePtr::NamedRef(name_ref) => {
                self.types.get_mut(*name_ref as usize).map(|t| &mut t.0)
            }
            types::TypePtr::Id(id) => {
                // @todo the item might not exist
                let item = self.blocks.get_item_mut(id)?;
                if let block::ItemContent::Type(t) = &mut item.content {
                    Some(t)
                } else {
                    None
                }
            }
            types::TypePtr::Named(name) => {
                let tref = self.get_type_ref(name)?;
                Some(&mut self.types[tref as usize].0)
            }
        }
    }

    pub fn init_type_from_ptr(&mut self, ptr: &types::TypePtr) -> Option<&types::Inner> {
        match ptr {
            types::TypePtr::Named(name) => {
                let id = self.init_type_ref(name.clone());
                self.types.get(id as usize).map(|t| &t.0)
            }
            _ => {
                if let Some(inner) = self.get_type(ptr) {
                    return Some(inner);
                } else {
                    None
                }
            }
        }
    }
    pub fn create_type_ptr(&mut self, name: &str) -> types::TypePtr {
        let id = self.init_type_ref(name.to_owned());
        types::TypePtr::NamedRef(id)
    }

    pub fn get_type_ref(&self, string: &str) -> Option<u32> {
        self.type_refs.get(string).map(|r| *r)
    }

    pub fn init_type_ref(&mut self, string: String) -> u32 {
        let types = &mut self.types;
        *self.type_refs.entry(string.to_owned()).or_insert_with(|| {
            let name_ref = types.len() as u32;
            let ptr = types::TypePtr::NamedRef(name_ref);
            let inner = types::Inner::new(ptr, None, types::TYPE_REFS_ARRAY);
            types.push((inner, string));
            name_ref
        })
    }

    pub fn get_type_name(&self, type_name_ref: u32) -> &str {
        &self.types[type_name_ref as usize].1
    }

    /// Compute a diff to sync with another client.
    ///
    /// This is the most efficient method to sync with another client by only
    /// syncing the differences.
    ///
    /// The sync protocol in Yrs/js is:
    /// * Send StateVector to the other client.
    /// * The other client comutes a minimal diff to sync by using the StateVector.
    pub fn encode_diff<E: Encoder>(&self, remote_sv: &StateVector, encoder: &mut E) {
        //TODO: this could be actually 2 steps:
        // 1. create Diff of block store and remote state vector (it can have lifetime of bock store)
        // 2. make Diff implement Encode trait and encode it
        // this way we can add some extra utility method on top of Diff (like introspection) without need of decoding it.
        self.write_blocks(remote_sv, encoder);
        let delete_set = DeleteSet::from(&self.blocks);
        delete_set.encode(encoder);
    }

    fn write_blocks<E: Encoder>(&self, remote_sv: &StateVector, encoder: &mut E) {
        let local_sv = self.blocks.get_state_vector();
        let mut diff = Self::diff_state_vectors(&local_sv, remote_sv);

        // Write items with higher client ids first
        // This heavily improves the conflict algorithm.
        diff.sort_by(|a, b| b.0.cmp(&a.0));

        encoder.write_uvar(diff.len());
        for (client, clock) in diff {
            let blocks = self.blocks.get(&client).unwrap();
            let clock = clock.max(blocks.first().id().clock); // make sure the first id exists
            let start = blocks.find_pivot(clock).unwrap();
            // write # encoded structs
            encoder.write_uvar(blocks.integrated_len() - start);
            encoder.write_client(client);
            encoder.write_uvar(clock);
            let first_block = &blocks[start];
            // write first struct with an offset
            first_block.encode(self, encoder);
            for i in (start + 1)..blocks.integrated_len() {
                blocks[i].encode(self, encoder);
            }
        }
    }

    fn diff_state_vectors(local_sv: &StateVector, remote_sv: &StateVector) -> Vec<(u64, u32)> {
        let mut diff = Vec::new();
        for (client, &remote_clock) in remote_sv.iter() {
            let local_clock = local_sv.get(client);
            if local_clock > remote_clock {
                diff.push((*client, remote_clock));
            }
        }
        for (client, _) in local_sv.iter() {
            if remote_sv.get(client) == 0 {
                diff.push((*client, 0));
            }
        }
        diff
    }
}

impl Encode for Store {
    /// Encodes the document state to a binary format.
    ///
    /// Document updates are idempotent and commutative. Caveats:
    /// * It doesn't matter in which order document updates are applied.
    /// * As long as all clients receive the same document updates, all clients
    ///   end up with the same content.
    /// * Even if an update contains known information, the unknown information
    ///   is extracted and integrated into the document structure.
    fn encode<E: Encoder>(&self, encoder: &mut E) {
        self.encode_diff(&StateVector::empty(), encoder)
    }
}
