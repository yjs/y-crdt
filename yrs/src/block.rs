use crate::store::Store;
use crate::types::TypePtr;
use crate::updates::decoder::Decoder;
use crate::updates::encoder::Encoder;
use crate::*;
use lib0::any::Any;
use std::collections::HashSet;
use std::hash::Hash;
use std::panic;

pub const BLOCK_GC_REF_NUMBER: u8 = 0;
pub const BLOCK_ITEM_DELETED_REF_NUMBER: u8 = 1;
pub const BLOCK_ITEM_JSON_REF_NUMBER: u8 = 2;
pub const BLOCK_ITEM_BINARY_REF_NUMBER: u8 = 3;
pub const BLOCK_ITEM_STRING_REF_NUMBER: u8 = 4;
pub const BLOCK_ITEM_EMBED_REF_NUMBER: u8 = 5;
pub const BLOCK_ITEM_FORMAT_REF_NUMBER: u8 = 6;
pub const BLOCK_ITEM_TYPE_REF_NUMBER: u8 = 7;
pub const BLOCK_ITEM_ANY_REF_NUMBER: u8 = 8;
pub const BLOCK_ITEM_DOC_REF_NUMBER: u8 = 9;
pub const BLOCK_SKIP_REF_NUMBER: u8 = 10;

pub const HAS_RIGHT_ORIGIN: u8 = 0b01000000;
pub const HAS_ORIGIN: u8 = 0b10000000;
pub const HAS_PARENT_SUB: u8 = 0b00100000;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ID {
    pub client: u64,
    pub clock: u32,
}

impl ID {
    pub fn new(client: u64, clock: u32) -> Self {
        ID { client, clock }
    }
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct BlockPtr {
    pub id: ID,
    pivot: u32,
}

impl BlockPtr {
    pub fn new(id: ID, pivot: u32) -> Self {
        BlockPtr { id, pivot }
    }

    pub fn from(id: ID) -> Self {
        BlockPtr {
            id,
            pivot: id.clock,
        }
    }

    #[inline]
    pub fn pivot(&self) -> usize {
        self.pivot as usize
    }

    pub fn fix_pivot(&self, pivot: u32) {
        unsafe { std::ptr::write(&self.pivot as *const u32 as *mut u32, pivot) };
    }
}

impl Eq for BlockPtr {}

impl PartialEq for BlockPtr {
    fn eq(&self, other: &Self) -> bool {
        // BlockPtr.pivot may differ, but logicaly it doesn't affect block equality
        self.id == other.id
    }
}

#[derive(Debug, PartialEq)]
pub enum Block {
    Item(Item),
    Skip(Skip),
    GC(GC),
}

impl Block {
    pub fn last_id(&self) -> ID {
        match self {
            Block::Item(item) => item.last_id(),
            Block::Skip(skip) => ID::new(skip.id.client, skip.id.clock + skip.len),
            Block::GC(gc) => ID::new(gc.id.client, gc.id.clock + gc.len),
        }
    }

    pub fn as_item(&self) -> Option<&Item> {
        match self {
            Block::Item(item) => Some(item),
            _ => None,
        }
    }

    pub fn as_item_mut(&mut self) -> Option<&mut Item> {
        match self {
            Block::Item(item) => Some(item),
            _ => None,
        }
    }

    pub fn is_deleted(&self) -> bool {
        match self {
            Block::Item(item) => item.deleted,
            Block::Skip(_) => false,
            Block::GC(_) => true,
        }
    }

    pub fn integrate(&mut self, txn: &mut Transaction<'_>, pivot: u32, offset: u32) {
        match self {
            Block::Item(item) => item.integrate(txn, pivot, offset),
            Block::GC(gc) => gc.integrate(offset),
            Block::Skip(_) => {
                panic!("Block::Skip cannot be integrated")
            }
        }
    }

    pub fn try_merge(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Block::Item(v1), Block::Item(v2)) => v1.try_merge(v2),
            (Block::GC(v1), Block::GC(v2)) => {
                v1.merge(v2);
                true
            }
            (Block::Skip(v1), Block::Skip(v2)) => {
                v1.merge(v2);
                true
            }
            _ => false,
        }
    }

    pub fn encode_with_offset<E: Encoder>(&self, encoder: &mut E, offset: u32) {
        if let Block::Item(item) = self {
            let origin = if offset > 0 {
                Some(ID::new(item.id.client, item.id.clock + offset - 1))
            } else {
                item.origin
            };
            let info = item.info();
            let cant_copy_parent_info = info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0;
            encoder.write_info(info);
            if let Some(origin_id) = origin {
                encoder.write_left_id(&origin_id);
            }
            if let Some(right_origin_id) = item.right_origin.as_ref() {
                encoder.write_right_id(right_origin_id);
            }
            if cant_copy_parent_info {
                match &item.parent {
                    types::TypePtr::Id(id) => {
                        encoder.write_parent_info(false);
                        encoder.write_left_id(&id.id);
                    }
                    types::TypePtr::Named(name) => {
                        encoder.write_parent_info(true);
                        encoder.write_string(name)
                    }
                    types::TypePtr::NamedRef(type_name_ref) => {
                        panic!("Block::encode_with_offset doesn't support named refs");
                    }
                }
                if let Some(parent_sub) = item.parent_sub.as_ref() {
                    encoder.write_string(parent_sub.as_str());
                }
            }
            item.content.encode_with_offset(encoder, offset);
        }
    }

    pub fn encode<E: Encoder>(&self, store: &Store, encoder: &mut E) {
        match self {
            Block::Item(item) => {
                let info = item.info();
                let cant_copy_parent_info = info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0;
                encoder.write_info(info);
                if let Some(origin_id) = item.origin.as_ref() {
                    encoder.write_left_id(origin_id);
                }
                if let Some(right_origin_id) = item.right_origin.as_ref() {
                    encoder.write_right_id(right_origin_id);
                }
                if cant_copy_parent_info {
                    match &item.parent {
                        types::TypePtr::NamedRef(type_name_ref) => {
                            let type_name = store.get_type_name(*type_name_ref);
                            encoder.write_parent_info(true);
                            encoder.write_string(type_name);
                        }
                        types::TypePtr::Id(id) => {
                            encoder.write_parent_info(false);
                            encoder.write_left_id(&id.id);
                        }
                        types::TypePtr::Named(name) => {
                            encoder.write_parent_info(true);
                            encoder.write_string(name)
                        }
                    }
                }
                if cant_copy_parent_info {
                    if let Some(parent_sub) = item.parent_sub.as_ref() {
                        encoder.write_string(parent_sub.as_str());
                    }
                }
                item.content.encode(encoder);
            }
            Block::Skip(skip) => {
                encoder.write_info(BLOCK_SKIP_REF_NUMBER);
                encoder.write_len(skip.len);
            }
            Block::GC(gc) => {
                encoder.write_info(BLOCK_GC_REF_NUMBER);
                encoder.write_len(gc.len);
            }
        }
    }

    pub fn id(&self) -> &ID {
        match self {
            Block::Item(item) => &item.id,
            Block::Skip(skip) => &skip.id,
            Block::GC(gc) => &gc.id,
        }
    }

    pub fn len(&self) -> u32 {
        match self {
            Block::Item(item) => item.content.len(),
            Block::Skip(skip) => skip.len,
            Block::GC(gc) => gc.len,
        }
    }

    pub fn clock_end(&self) -> u32 {
        match self {
            Block::Item(item) => item.id.clock + item.content.len(),
            Block::Skip(skip) => skip.id.clock + skip.len,
            Block::GC(gc) => gc.id.clock + gc.len,
        }
    }

    /// Returns an ID of a block, current item depends upon
    /// (meaning: dependency must appear in the store before current item).
    pub fn dependency(&self) -> Option<&ID> {
        match self {
            Block::Item(item) => item.dependency(),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct ItemPosition {
    pub parent: types::TypePtr,
    pub left: Option<BlockPtr>,
    pub right: Option<BlockPtr>,
    pub index: u32,
}

#[derive(Debug, PartialEq)]
pub struct Item {
    pub id: ID,
    pub left: Option<BlockPtr>,
    pub right: Option<BlockPtr>,
    pub origin: Option<ID>,
    pub right_origin: Option<ID>,
    pub content: ItemContent,
    pub parent: types::TypePtr,
    pub parent_sub: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Skip {
    pub id: ID,
    pub len: u32,
}

impl Skip {
    pub fn new(id: ID, len: u32) -> Self {
        Skip { id, len }
    }
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.len += other.len;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct GC {
    pub id: ID,
    pub len: u32,
}

impl GC {
    pub fn new(id: ID, len: u32) -> Self {
        GC { id, len }
    }

    pub fn integrate(&mut self, pivot: u32) {
        if pivot > 0 {
            self.id.clock += pivot;
            self.len -= pivot;
        }
    }

    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.len += other.len;
    }
}

impl Item {
    pub(crate) fn mark_as_deleted(&self) {
        unsafe {
            let ptr = &self.deleted as *const bool as *mut bool;
            std::ptr::write(ptr, true);
        }
    }

    /// Assign left/right neighbors of the block. This may require for origin/right_origin
    /// blocks to be already present in block store - which may not be the case during block
    /// decoding. We decode entire update first, and apply individual blocks second, hence
    /// repair function is called before applying the block rather than on decode.
    pub fn repair(&mut self, txn: &mut Transaction<'_>) {
        if let Some(origin_id) = self.origin {
            let ptr = BlockPtr::from(origin_id);
            if let Some(item) = txn.store.blocks.get_item(&ptr) {
                let id = self.origin.unwrap();
                let len = item.len();
                if id.clock == item.id.clock + len - 1 {
                    self.left = Some(ptr);
                } else {
                    let ptr =
                        BlockPtr::new(ID::new(origin_id.client, origin_id.clock + 1), ptr.pivot);
                    let (l, _) = txn.store.blocks.split_block(&ptr);
                    self.left = l;
                }
            }
        }

        if let Some(id) = self.right_origin {
            let (l, r) = txn.store.blocks.split_block(&BlockPtr::from(id));
            // if we got a split, point to right-side
            // if right side is None, then no split happened and `l` is right neighbor
            self.right = r.or(l);
        }
    }

    pub fn integrate(&mut self, txn: &mut Transaction<'_>, pivot: u32, offset: u32) {
        if offset > 0 {
            self.id.clock += offset;
            let (left, _) = txn
                .store
                .blocks
                .split_block(&BlockPtr::from(ID::new(self.id.client, self.id.clock - 1)));
            if let Some(mut left) = left {
                if let Some(origin) = txn.store.blocks.get_item(&left) {
                    self.origin = Some(origin.last_id());
                    left.id = origin.last_id();
                    self.left = Some(left);
                }
            } else {
                self.left = None;
                self.origin = None;
            }
            self.content.splice(offset as usize);
        }

        // In the original Y.js algorithm we decoded items as we go and attached them to client
        // block list. During that process if we had right origin but no left, we made a lookup for
        // right origin's parent and attach it as a parent of current block.
        //
        // Here since we decode all blocks first, then apply them, we might not find them in
        // the block store during decoding. Therefore we retroactively reattach it here.
        if let TypePtr::Id(ptr) = &self.parent {
            if Some(ptr.id) == self.right_origin {
                if let Some(item) = txn.store.blocks.get_item(&ptr) {
                    self.parent = item.parent.clone();
                }
            }
        }

        let left = self
            .left
            .as_ref()
            .and_then(|ptr| txn.store.blocks.get_block(ptr));
        let right = self
            .right
            .as_ref()
            .and_then(|ptr| txn.store.blocks.get_block(ptr));

        let right_is_null_or_has_left = match right {
            None => true,
            Some(Block::Item(i)) => i.left.is_some(),
            _ => false,
        };
        let left_has_other_right_than_self = match left {
            Some(Block::Item(i)) => i.right != self.right,
            _ => false,
        };

        if (left.is_none() && right_is_null_or_has_left) || left_has_other_right_than_self {
            // set the first conflicting item
            let mut o = if let Some(Block::Item(left)) = left {
                left.right
            } else if let Some(sub) = &self.parent_sub {
                if let Some(parent) = txn.store.get_type(&self.parent) {
                    let mut o = parent.map.get(sub);
                    while let Some(ptr) = o {
                        if let Some(item) = txn.store.blocks.get_item(ptr) {
                            o = item.left.as_ref();
                        } else {
                            break;
                        }
                    }
                    o.cloned()
                } else {
                    None
                }
            } else {
                if let Some(parent) = txn.store.get_type(&self.parent) {
                    parent.start.get()
                } else {
                    self.right.clone()
                }
            };

            let mut left = self.left.clone();
            let mut conflicting_items = HashSet::new();
            let mut items_before_origin = HashSet::new();

            // Let c in conflicting_items, b in items_before_origin
            // ***{origin}bbbb{this}{c,b}{c,b}{o}***
            // Note that conflicting_items is a subset of items_before_origin
            while let Some(ptr) = o {
                if Some(ptr) == self.right {
                    break;
                }

                items_before_origin.insert(ptr.id.clone());
                conflicting_items.insert(ptr.id.clone());
                if let Some(Block::Item(item)) = txn.store.blocks.get_block(&ptr) {
                    if self.origin == item.origin {
                        // case 1
                        if ptr.id.client < self.id.client {
                            left = Some(ptr.clone());
                            conflicting_items.clear();
                        } else if self.right_origin == item.right_origin {
                            // `self` and `item` are conflicting and point to the same integration
                            // points. The id decides which item comes first. Since `self` is to
                            // the left of `item`, we can break here.
                            break;
                        }
                    } else {
                        if let Some(item_origin) = item.origin {
                            if items_before_origin.contains(&item_origin) {
                                if !conflicting_items.contains(&item_origin) {
                                    left = Some(ptr.clone());
                                    conflicting_items.clear();
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    o = item.right.clone();
                } else {
                    break;
                }
            }
            self.left = left;
        }

        // reconnect left/right
        if let Some(left_id) = self.left.as_ref() {
            if let Some(left) = txn.store.blocks.get_item_mut(left_id) {
                self.right = left.right.replace(BlockPtr::new(self.id, pivot));
            }
        } else {
            let r = if let Some(parent_sub) = &self.parent_sub {
                let start = match txn.store.get_type(&self.parent) {
                    None => txn
                        .store
                        .init_type_from_ptr(&self.parent)
                        .and_then(|p| p.map.get(parent_sub)),
                    Some(parent) => parent.map.get(parent_sub),
                }
                .cloned();
                let mut o = start.as_ref();

                while let Some(ptr) = o {
                    if let Some(item) = txn.store.blocks.get_item(ptr) {
                        o = item.left.as_ref();
                    } else {
                        break;
                    }
                }
                o.cloned()
            } else {
                let parent_type = txn.store.init_type_from_ptr(&self.parent).unwrap();
                let start = parent_type
                    .start
                    .replace(Some(BlockPtr::new(self.id, pivot)));
                start
            };
            self.right = r;
        }

        if let Some(right_id) = self.right.as_ref() {
            if let Some(right) = txn.store.blocks.get_item_mut(right_id) {
                right.left = Some(BlockPtr::new(self.id, pivot));
            }
        } else if let Some(parent_sub) = &self.parent_sub {
            // set as current parent value if right === null and this is parentSub
            if let Some(parent) = txn.store.get_type_mut(&self.parent) {
                let ptr = BlockPtr::new(self.id, pivot);
                parent.map.insert(parent_sub.clone(), ptr);
            }
            if let Some(left) = self.left {
                // this is the current attribute value of parent. delete right
                if let Some(item) = txn.store.blocks.get_item_mut(&left) {
                    //item.delete(txn);
                    todo!()
                }
            }
        }

        self.integrate_content(txn);
        // addChangedTypeToTransaction(transaction, /** @type {AbstractType<any>} */ (this.parent), this.parentSub)
        let parent_deleted = false; // (this.parent)._item !== null && (this.parent)._item.deleted)
        if parent_deleted || (self.parent_sub.is_some() && self.right.is_some()) {
            // delete if parent is deleted or if this is not the current attribute value of parent
            self.delete(txn);
        }
    }

    pub fn delete(&mut self, txn: &mut Transaction<'_>) {
        if !self.deleted {
            self.mark_as_deleted();
            self.content.delete(txn);
            txn.delete_set.insert(self.id, self.len());
            //TODO: addChangedTypeToTransaction(transaction, parent, this.parentSub)
        }
    }

    pub fn len(&self) -> u32 {
        self.content.len()
    }

    pub fn split(&mut self, diff: u32) -> Item {
        let client = self.id.client;
        let clock = self.id.clock;
        let other = Item {
            id: ID::new(client, clock + diff),
            left: Some(BlockPtr::from(ID::new(client, clock + diff - 1))),
            right: self.right.clone(),
            origin: Some(ID::new(client, clock + diff - 1)),
            right_origin: self.right_origin.clone(),
            content: self.content.splice(diff as usize).unwrap(),
            parent: self.parent.clone(),
            parent_sub: self.parent_sub.clone(),
            deleted: self.deleted,
        };
        self.right = Some(BlockPtr::from(other.id));
        other
    }

    pub fn last_id(&self) -> ID {
        ID::new(self.id.client, self.id.clock + self.len() - 1)
    }

    /// Tries to merge current [Item] with another, returning true if merge was performed successfully.
    pub fn try_merge(&mut self, other: &Self) -> bool {
        if self.id.client == other.id.client
            && self.id.clock + self.len() == other.id.clock
            && other.origin == Some(self.last_id())
            && self.right == Some(BlockPtr::from(other.id.clone()))
            && self.right_origin == other.right_origin
            && self.deleted == other.deleted
            && self.content.try_merge(&other.content)
        {
            true
        } else {
            false
        }
    }

    /// Returns an ID of a block, current item depends upon
    /// (meaning: dependency must appear in the store before current item).
    pub fn dependency(&self) -> Option<&ID> {
        self.origin
            .as_ref()
            .or_else(|| self.right_origin.as_ref())
            .or_else(|| match &self.parent {
                TypePtr::Id(ptr) => Some(&ptr.id),
                _ => None,
            })
    }

    fn info(&self) -> u8 {
        let info = if self.origin.is_some() { HAS_ORIGIN } else { 0 } // is left null
            | if self.right_origin.is_some() { HAS_RIGHT_ORIGIN } else { 0 } // is right null
            | if self.parent_sub.is_some() { HAS_PARENT_SUB } else { 0 }
            | (self.content.get_ref_number() & 0b1111);
        info
    }

    fn integrate_content(&mut self, txn: &mut Transaction<'_>) {
        match &mut self.content {
            ItemContent::Deleted(len) => {
                txn.delete_set.insert(self.id, *len);
                self.mark_as_deleted();
            }
            ItemContent::Doc(_, _) => {
                //// this needs to be reflected in doc.destroy as well
                //this.doc._item = item
                //transaction.subdocsAdded.add(this.doc)
                //if (this.doc.shouldLoad) {
                //    transaction.subdocsLoaded.add(this.doc)
                //}
                todo!()
            }
            ItemContent::Format(_, _) => {
                // @todo searchmarker are currently unsupported for rich text documents
                // /** @type {AbstractType<any>} */ (item.parent)._searchMarker = null
            }
            ItemContent::Type(_) => {
                // this.type._integrate(transaction.doc, item)
                todo!()
            }
            _ => {
                // other types don't define integration-specific actions
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ItemContent {
    Any(Vec<Any>),
    Binary(Vec<u8>),
    Deleted(u32),
    Doc(String, Any),
    JSON(Vec<String>),      // String is JSON
    Embed(String),          // String is JSON
    Format(String, String), // key, value: JSON
    String(String),
    Type(types::Inner),
}

impl ItemContent {
    pub fn get_ref_number(&self) -> u8 {
        match self {
            ItemContent::Any(_) => BLOCK_ITEM_ANY_REF_NUMBER,
            ItemContent::Binary(_) => BLOCK_ITEM_BINARY_REF_NUMBER,
            ItemContent::Deleted(_) => BLOCK_ITEM_DELETED_REF_NUMBER,
            ItemContent::Doc(_, _) => BLOCK_ITEM_DOC_REF_NUMBER,
            ItemContent::JSON(_) => BLOCK_ITEM_JSON_REF_NUMBER,
            ItemContent::Embed(_) => BLOCK_ITEM_EMBED_REF_NUMBER,
            ItemContent::Format(_, _) => BLOCK_ITEM_FORMAT_REF_NUMBER,
            ItemContent::String(_) => BLOCK_ITEM_STRING_REF_NUMBER,
            ItemContent::Type(_) => BLOCK_ITEM_TYPE_REF_NUMBER,
        }
    }

    fn delete(&mut self, _txn: &mut Transaction<'_>) {
        match self {
            ItemContent::Doc(_, _) => {
                //if (transaction.subdocsAdded.has(this.doc)) {
                //    transaction.subdocsAdded.delete(this.doc)
                //} else {
                //    transaction.subdocsRemoved.add(this.doc)
                //}
                todo!()
            }
            ItemContent::Type(_) => {
                //let item = this.type._start
                //while (item !== null) {
                //    if (!item.deleted) {
                //        item.delete(transaction)
                //    } else {
                //        // Whis will be gc'd later and we want to merge it if possible
                //        // We try to merge all deleted items after each transaction,
                //        // but we have no knowledge about that this needs to be merged
                //        // since it is not in transaction.ds. Hence we add it to transaction._mergeStructs
                //        transaction._mergeStructs.push(item)
                //    }
                //    item = item.right
                //}
                //this.type._map.forEach(item => {
                //    if (!item.deleted) {
                //        item.delete(transaction)
                //    } else {
                //        // same as above
                //        transaction._mergeStructs.push(item)
                //    }
                //})
                //transaction.changed.delete(this.type)
                todo!()
            }
            _ => {
                // nothing to do for other content types
            }
        }
    }

    pub fn len(&self) -> u32 {
        match self {
            ItemContent::Deleted(deleted) => *deleted,
            ItemContent::String(str) => {
                // @todo this should return the length in utf16!
                str.len() as u32
            }
            _ => 1,
        }
    }

    pub fn get_content(&self) -> Vec<Any> {
        match self {
            ItemContent::Any(v) => v.clone(),
            ItemContent::Binary(v) => vec![Any::Buffer(v.clone().into_boxed_slice())],
            ItemContent::Deleted(_) => Vec::default(),
            ItemContent::Doc(_, v) => vec![v.clone()],
            ItemContent::JSON(v) => v.iter().map(|v| Any::String(v.clone())).collect(),
            ItemContent::Embed(v) => vec![Any::String(v.clone())],
            ItemContent::Format(_, _) => Vec::default(),
            ItemContent::String(v) => v.chars().map(|c| Any::String(c.to_string())).collect(),
            ItemContent::Type(_) => panic!("ItemContent::get_content on type?"),
        }
    }

    /// Similar to [get_content], but it only returns the latest result and doesn't materialize
    /// other for performance reasons.
    pub fn value(&self) -> Option<Any> {
        match self {
            ItemContent::Any(v) => v.last().cloned(),
            ItemContent::Binary(v) => Some(Any::Buffer(v.clone().into_boxed_slice())),
            ItemContent::Deleted(_) => None,
            ItemContent::Doc(_, v) => Some(v.clone()),
            ItemContent::JSON(v) => v.last().map(|v| Any::String(v.clone())),
            ItemContent::Embed(v) => Some(Any::String(v.clone())),
            ItemContent::Format(_, _) => None,
            ItemContent::String(v) => Some(Any::String(v.clone())),
            ItemContent::Type(inner) => panic!("ItemContent::value on type?"),
        }
    }

    pub fn encode_with_offset<E: Encoder>(&self, encoder: &mut E, offset: u32) {
        match self {
            ItemContent::Deleted(len) => encoder.write_len(*len - offset),
            ItemContent::Binary(buf) => encoder.write_buf(buf),
            ItemContent::String(s) => encoder.write_string(&s.as_str()[(offset as usize)..]),
            ItemContent::Embed(s) => encoder.write_string(s.as_str()),
            ItemContent::JSON(s) => {
                encoder.write_len(s.len() as u32 - offset);
                for i in (offset as usize)..s.len() {
                    encoder.write_string(s[i].as_str())
                }
            }
            ItemContent::Format(k, v) => {
                encoder.write_string(k.as_str());
                encoder.write_string(v.as_str());
            }
            ItemContent::Type(inner) => {
                encoder.write_type_ref(inner.type_ref);
                if inner.type_ref == types::TYPE_REFS_XML_ELEMENT
                    || inner.type_ref == types::TYPE_REFS_XML_HOOK
                {
                    encoder.write_key(inner.name.as_ref().unwrap().as_str())
                }
            }
            ItemContent::Any(any) => {
                encoder.write_len(any.len() as u32 - offset);
                for i in (offset as usize)..any.len() {
                    encoder.write_any(&any[i]);
                }
            }
            ItemContent::Doc(key, any) => {
                encoder.write_string(key.as_str());
                encoder.write_any(any);
            }
        }
    }

    pub fn encode<E: Encoder>(&self, encoder: &mut E) {
        match self {
            ItemContent::Deleted(len) => encoder.write_len(*len),
            ItemContent::Binary(buf) => encoder.write_buf(buf),
            ItemContent::String(s) => encoder.write_string(s.as_str()),
            ItemContent::Embed(s) => encoder.write_string(s.as_str()),
            ItemContent::JSON(s) => {
                encoder.write_len(s.len() as u32);
                for json in s.iter() {
                    encoder.write_string(json.as_str())
                }
            }
            ItemContent::Format(k, v) => {
                encoder.write_string(k.as_str());
                encoder.write_string(v.as_str());
            }
            ItemContent::Type(inner) => {
                encoder.write_type_ref(inner.type_ref);
                if inner.type_ref == types::TYPE_REFS_XML_ELEMENT
                    || inner.type_ref == types::TYPE_REFS_XML_HOOK
                {
                    encoder.write_key(inner.name.as_ref().unwrap().as_str())
                }
            }
            ItemContent::Any(any) => {
                encoder.write_len(any.len() as u32);
                for a in any.iter() {
                    encoder.write_any(a);
                }
            }
            ItemContent::Doc(key, any) => {
                encoder.write_string(key.as_str());
                encoder.write_any(any);
            }
        }
    }

    pub fn decode<D: Decoder>(decoder: &mut D, ref_num: u8, ptr: block::BlockPtr) -> Self {
        match ref_num & 0b1111 {
            BLOCK_ITEM_DELETED_REF_NUMBER => ItemContent::Deleted(decoder.read_len()),
            BLOCK_ITEM_JSON_REF_NUMBER => {
                let mut remaining = decoder.read_len() as i32;
                let mut buf = Vec::with_capacity(remaining as usize);
                while remaining >= 0 {
                    buf.push(decoder.read_string().to_owned());
                    remaining -= 1;
                }
                ItemContent::JSON(buf)
            }
            BLOCK_ITEM_BINARY_REF_NUMBER => ItemContent::Binary(decoder.read_buf().to_owned()),
            BLOCK_ITEM_STRING_REF_NUMBER => ItemContent::String(decoder.read_string().to_owned()),
            BLOCK_ITEM_EMBED_REF_NUMBER => ItemContent::Embed(decoder.read_string().to_owned()),
            BLOCK_ITEM_FORMAT_REF_NUMBER => ItemContent::Format(
                decoder.read_string().to_owned(),
                decoder.read_string().to_owned(),
            ),
            BLOCK_ITEM_TYPE_REF_NUMBER => {
                let type_ref = decoder.read_type_ref();
                let name = if type_ref == types::TYPE_REFS_XML_ELEMENT
                    || type_ref == types::TYPE_REFS_XML_HOOK
                {
                    Some(decoder.read_key().to_owned())
                } else {
                    None
                };
                let inner_ptr = types::TypePtr::Id(ptr);
                let inner = types::Inner::new(inner_ptr, name, type_ref);
                ItemContent::Type(inner)
            }
            BLOCK_ITEM_ANY_REF_NUMBER => {
                let len = decoder.read_len() as usize;
                let mut values = Vec::with_capacity(len);
                let mut i = 0;
                while i < len {
                    values.push(decoder.read_any());
                    i += 1;
                }
                ItemContent::Any(values)
            }
            BLOCK_ITEM_DOC_REF_NUMBER => {
                ItemContent::Doc(decoder.read_string().to_owned(), decoder.read_any())
            }
            info => panic!("ItemContent::decode unrecognized info flag: {}", info),
        }
    }

    pub(crate) fn splice(&mut self, offset: usize) -> Option<ItemContent> {
        match self {
            ItemContent::Any(value) => {
                let (left, right) = value.split_at(offset);
                let left = left.to_vec();
                let right = right.to_vec();
                *self = ItemContent::Any(left);
                Some(ItemContent::Any(right))
            }
            ItemContent::String(string) => {
                let (left, right) = string.split_at(offset);
                let left = left.to_string();
                let right = right.to_string();

                //TODO: do we need that in Rust?
                //let split_point = left.chars().last().unwrap();
                //if split_point >= 0xD800 as char && split_point <= 0xDBFF as char {
                //    // Last character of the left split is the start of a surrogate utf16/ucs2 pair.
                //    // We don't support splitting of surrogate pairs because this may lead to invalid documents.
                //    // Replace the invalid character with a unicode replacement character (� / U+FFFD)
                //    left.replace_range((offset-1)..offset, "�");
                //    right.replace_range(0..1, "�");
                //}
                *self = ItemContent::String(left);

                Some(ItemContent::String(right))
            }
            ItemContent::Deleted(len) => {
                let right = ItemContent::Deleted(*len - offset as u32);
                *len = offset as u32;
                Some(right)
            }
            ItemContent::JSON(value) => {
                let (left, right) = value.split_at(offset);
                let left = left.to_vec();
                let right = right.to_vec();
                *self = ItemContent::JSON(left);
                Some(ItemContent::JSON(right))
            }
            _ => None,
        }
    }

    pub fn try_merge(&mut self, other: &Self) -> bool {
        //TODO: change `other` to Self (not ref) and return type to Option<Self> (none if merge suceeded)
        match (self, other) {
            (ItemContent::Any(v1), ItemContent::Any(v2)) => {
                v1.append(&mut v2.clone());
                true
            }
            (ItemContent::Deleted(v1), ItemContent::Deleted(v2)) => {
                *v1 = *v1 + *v2;
                true
            }
            (ItemContent::JSON(v1), ItemContent::JSON(v2)) => {
                v1.append(&mut v2.clone());
                true
            }
            (ItemContent::String(v1), ItemContent::String(v2)) => {
                v1.push_str(v2.as_str());
                true
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for ID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{}#{}>", self.client, self.clock)
    }
}

impl std::fmt::Display for BlockPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}->{})", self.id, self.pivot())
    }
}

impl std::fmt::Display for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Block::Item(item) = self {
            item.fmt(f)
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Display for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}", self.id)?;
        if let Some(origin) = self.origin.as_ref() {
            write!(f, ", origin-l: {}", origin)?;
        }
        if let Some(origin) = self.right_origin.as_ref() {
            write!(f, ", origin-r: {}", origin)?;
        }
        if let Some(left) = self.left.as_ref() {
            write!(f, ", left: {}", left.id)?;
        }
        if let Some(right) = self.right.as_ref() {
            write!(f, ", right: {}", right.id)?;
        }
        if self.deleted {
            write!(f, ": ~{}~)", &self.content)
        } else {
            write!(f, ": '{}')", &self.content)
        }
    }
}

impl std::fmt::Display for ItemContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemContent::String(s) => write!(f, "{}", s),
            _ => Ok(()),
        }
    }
}

impl std::fmt::Display for ItemPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "(index: {}", self.index)?;
        if let Some(l) = self.left.as_ref() {
            write!(f, ", left: {}", l)?;
        }
        if let Some(r) = self.right.as_ref() {
            write!(f, ", right: {}", r)?;
        }
        write!(f, ")")
    }
}

#[cfg(test)]
mod test {
    use crate::block::BlockPtr;
    use crate::ID;

    #[test]
    fn block_ptr_pivot() {
        let ptr = BlockPtr::new(ID::new(1, 2), 3);
        assert_eq!(ptr.pivot(), 3);
        ptr.fix_pivot(4);
        assert_eq!(ptr.pivot(), 4);
    }
}
