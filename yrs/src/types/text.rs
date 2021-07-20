use crate::block::BlockPtr;
use crate::transaction::Transaction;
use crate::*;

pub struct Text {
    ptr: types::TypePtr,
}

impl Text {
    pub fn from(ptr: types::TypePtr) -> Self {
        Text { ptr }
    }
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self, tr: &Transaction) -> String {
        tr.store
            .get_type(&self.ptr)
            .and_then(|inner| {
                let mut start = inner.start.get();
                let mut s = String::new();
                while let Some(a) = start.as_ref() {
                    if let Some(item) = tr.store.blocks.get_item(&a) {
                        if !item.deleted {
                            if let block::ItemContent::String(item_string) = &item.content {
                                s.push_str(item_string);
                            }
                        }
                        start = item.right.clone();
                    } else {
                        break;
                    }
                }
                Some(s)
            })
            .unwrap_or_default()
    }

    fn find_position(&self, txn: &mut Transaction, mut count: u32) -> Option<block::ItemPosition> {
        let mut pos = {
            let inner = txn.store.get_type(&self.ptr)?;
            block::ItemPosition {
                parent: inner.ptr.clone(),
                left: None,
                right: inner.start.get(),
                index: 0,
            }
        };

        while let Some(right_ptr) = pos.right.as_ref() {
            if count == 0 {
                break;
            }

            if let Some(mut right) = txn.store.blocks.get_item(right_ptr) {
                if !right.deleted {
                    let mut right_len = right.len();
                    if count < right_len {
                        // split right item
                        let split_ptr = BlockPtr::new(
                            ID::new(right.id.client, right.id.clock + count),
                            right_ptr.pivot() as u32,
                        );
                        let (_, _) = txn.store.blocks.split_block(&split_ptr);
                        right = txn.store.blocks.get_item(right_ptr).unwrap();
                        right_len = right.len();
                    }
                    pos.index += right_len;
                    count -= right_len;
                }
                pos.left = pos.right.take();
                pos.right = right.right.clone();
            } else {
                return None;
            }
        }

        Some(pos)
    }

    pub fn insert(&self, tr: &mut Transaction, index: u32, content: &str) {
        if let Some(pos) = self.find_position(tr, index) {
            tr.create_item(&pos, block::ItemContent::String(content.to_owned()), None);
        } else {
            panic!("The type or the position doesn't exist!");
        }
    }

    pub fn delete(&self, txn: &mut Transaction, index: u32, mut len: u32) {
        if let Some(pos) = self.find_position(txn, index) {
            let mut current = {
                let block = pos
                    .left
                    .as_ref()
                    .and_then(|ptr| txn.store.blocks.get_block(ptr));
                match block {
                    Some(block) => block
                        .as_item()
                        .and_then(|item| item.right.as_ref())
                        .and_then(|ptr| txn.store.blocks.get_item(ptr)),
                    None => {
                        let ptr = txn.store.get_type(&pos.parent).unwrap().start.get();
                        let item = ptr.as_ref().and_then(|ptr| txn.store.blocks.get_item(ptr));
                        item
                    }
                }
            };
            while let Some(mut item) = current {
                if len == 0 {
                    break;
                }
                if !item.deleted {
                    if len < item.len() {
                        // split item
                        let mut split_ptr = BlockPtr::from(item.id);
                        split_ptr.id.clock += len;
                        let (left, _) = txn.store.blocks.split_block(&split_ptr);
                        item = txn
                            .store
                            .blocks
                            .get_block(left.as_ref().unwrap())
                            .unwrap()
                            .as_item()
                            .unwrap();
                    }

                    len -= item.len();
                    item.mark_as_deleted();
                }
                current = item
                    .right
                    .as_ref()
                    .and_then(|ptr| txn.store.blocks.get_block(ptr))
                    .and_then(|block| block.as_item());
            }
        } else {
            panic!("The type or the position doesn't exist!");
        }
    }
}

#[cfg(test)]
mod test {
    use crate::Doc;

    #[test]
    fn append_single_character_blocks() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "a");
        txt.insert(&mut txn, 1, "b");
        txt.insert(&mut txn, 2, "c");

        assert_eq!(txt.to_string(&txn).as_str(), "abc");
    }

    #[test]
    fn append_mutli_character_blocks() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "hello");
        txt.insert(&mut txn, 5, " ");
        txt.insert(&mut txn, 6, "world");

        assert_eq!(txt.to_string(&txn).as_str(), "hello world");
    }

    #[test]
    fn prepend_single_character_blocks() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "a");
        txt.insert(&mut txn, 0, "b");
        txt.insert(&mut txn, 0, "c");

        assert_eq!(txt.to_string(&txn).as_str(), "cba");
    }

    #[test]
    fn prepend_mutli_character_blocks() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "hello");
        txt.insert(&mut txn, 0, " ");
        txt.insert(&mut txn, 0, "world");

        assert_eq!(txt.to_string(&txn).as_str(), "world hello");
    }

    #[test]
    fn insert_after_block() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "hello");
        txt.insert(&mut txn, 5, " ");
        txt.insert(&mut txn, 6, "world");
        txt.insert(&mut txn, 6, "beautiful ");

        assert_eq!(txt.to_string(&txn).as_str(), "hello beautiful world");
    }

    #[test]
    fn insert_inside_of_block() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "it was expected");
        txt.insert(&mut txn, 6, " not");

        assert_eq!(txt.to_string(&txn).as_str(), "it was not expected");
    }

    #[test]
    fn insert_concurrent_root() {
        let d1 = Doc::with_client_id(1);
        let mut t1 = d1.transact();
        let txt1 = t1.get_text("test");

        txt1.insert(&mut t1, 0, "hello ");

        let d2 = Doc::with_client_id(2);
        let mut t2 = d2.transact();
        let txt2 = t2.get_text("test");

        txt2.insert(&mut t2, 0, "world");

        let d1_sv = d1.get_state_vector(&t1);
        let d2_sv = d2.get_state_vector(&t2);

        let u1 = d1.encode_delta_as_update(&d2_sv, &t1);
        let u2 = d2.encode_delta_as_update(&d1_sv, &t2);

        d1.apply_update(&mut t1, u2.as_slice());
        d2.apply_update(&mut t2, u1.as_slice());

        let a = txt1.to_string(&t1);
        let b = txt2.to_string(&t2);

        assert_eq!(a, b);
        assert_eq!(a.as_str(), "hello world");
    }

    #[test]
    fn insert_concurrent_in_the_middle() {
        let d1 = Doc::with_client_id(1);
        let mut t1 = d1.transact();
        let txt1 = t1.get_text("test");

        txt1.insert(&mut t1, 0, "I expect that");
        assert_eq!(txt1.to_string(&t1).as_str(), "I expect that");

        let d2 = Doc::with_client_id(2);
        let mut t2 = d2.transact();

        let d2_sv = d2.get_state_vector(&t2);
        let u1 = d1.encode_delta_as_update(&d2_sv, &t1);
        d2.apply_update(&mut t2, u1.as_slice());

        let txt2 = t2.get_text("test");
        assert_eq!(txt2.to_string(&t2).as_str(), "I expect that");

        txt2.insert(&mut t2, 1, " have");
        txt2.insert(&mut t2, 13, "ed");
        assert_eq!(txt2.to_string(&t2).as_str(), "I have expected that");

        txt1.insert(&mut t1, 1, " didn't");
        assert_eq!(txt1.to_string(&t1).as_str(), "I didn't expect that");

        let d2_sv = d2.get_state_vector(&t2);
        let d1_sv = d1.get_state_vector(&t1);
        let u1 = d1.encode_delta_as_update(&d2_sv, &t1);
        let u2 = d2.encode_delta_as_update(&d1_sv, &t2);
        d1.apply_update(&mut t1, u2.as_slice());
        d2.apply_update(&mut t2, u1.as_slice());

        let a = txt1.to_string(&t1);
        let b = txt2.to_string(&t2);

        assert_eq!(a, b);
        assert_eq!(a.as_str(), "I didn't have expected that");
    }

    #[test]
    fn append_concurrent() {
        let d1 = Doc::with_client_id(1);
        let mut t1 = d1.transact();
        let txt1 = t1.get_text("test");

        txt1.insert(&mut t1, 0, "aaa");
        assert_eq!(txt1.to_string(&t1).as_str(), "aaa");

        let d2 = Doc::with_client_id(2);
        let mut t2 = d2.transact();

        let d2_sv = d2.get_state_vector(&t2);
        let u1 = d1.encode_delta_as_update(&d2_sv, &t1);
        d2.apply_update(&mut t2, u1.as_slice());

        let txt2 = t2.get_text("test");
        assert_eq!(txt2.to_string(&t2).as_str(), "aaa");

        txt2.insert(&mut t2, 3, "bbb");
        txt2.insert(&mut t2, 6, "bbb");
        assert_eq!(txt2.to_string(&t2).as_str(), "aaabbbbbb");

        txt1.insert(&mut t1, 3, "aaa");
        assert_eq!(txt1.to_string(&t1).as_str(), "aaaaaa");

        let d2_sv = d2.get_state_vector(&t2);
        let d1_sv = d1.get_state_vector(&t1);
        let u1 = d1.encode_delta_as_update(&d2_sv, &t1);
        let u2 = d2.encode_delta_as_update(&d1_sv, &t2);

        d1.apply_update(&mut t1, u2.as_slice());
        d2.apply_update(&mut t2, u1.as_slice());

        let a = txt1.to_string(&t1);
        let b = txt2.to_string(&t2);

        assert_eq!(a.as_str(), "aaaaaabbbbbb");
        assert_eq!(a, b);
    }

    #[test]
    fn delete_single_block_start() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "bbb");
        txt.insert(&mut txn, 0, "aaa");
        txt.delete(&mut txn, 0, 3);

        assert_eq!(txt.to_string(&txn).as_str(), "bbb");
    }

    #[test]
    fn delete_single_block_end() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "bbb");
        txt.insert(&mut txn, 0, "aaa");
        txt.delete(&mut txn, 3, 3);

        assert_eq!(txt.to_string(&txn).as_str(), "aaa");
    }

    #[test]
    fn delete_multiple_whole_blocks() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "a");
        txt.insert(&mut txn, 1, "b");
        txt.insert(&mut txn, 2, "c");

        txt.delete(&mut txn, 1, 1);
        assert_eq!(txt.to_string(&txn).as_str(), "ac");

        txt.delete(&mut txn, 1, 1);
        assert_eq!(txt.to_string(&txn).as_str(), "a");

        txt.delete(&mut txn, 0, 1);
        assert_eq!(txt.to_string(&txn).as_str(), "");
    }

    #[test]
    fn delete_slice_of_block() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "abc");
        txt.delete(&mut txn, 1, 1);

        assert_eq!(txt.to_string(&txn).as_str(), "ac");
    }

    #[test]
    fn delete_multiple_blocks_with_slicing() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "hello ");
        txt.insert(&mut txn, 6, "beautiful");
        txt.insert(&mut txn, 15, " world");

        txt.delete(&mut txn, 5, 11);
        assert_eq!(txt.to_string(&txn).as_str(), "helloworld");
    }

    #[test]
    fn insert_after_delete() {
        let doc = Doc::new();
        let mut txn = doc.transact();
        let txt = txn.get_text("test");

        txt.insert(&mut txn, 0, "hello ");
        txt.delete(&mut txn, 0, 5);
        txt.insert(&mut txn, 1, "world");

        assert_eq!(txt.to_string(&txn).as_str(), " world");
    }

    #[test]
    fn concurrent_insert_delete() {
        let d1 = Doc::with_client_id(1);
        let mut t1 = d1.transact();
        let txt1 = t1.get_text("test");

        txt1.insert(&mut t1, 0, "hello world");
        assert_eq!(txt1.to_string(&t1).as_str(), "hello world");

        let u1 = d1.encode_state_as_update(&t1);

        let d2 = Doc::with_client_id(2);
        let mut t2 = d2.transact();
        d2.apply_update(&mut t2, u1.as_slice());
        let txt2 = t2.get_text("test");
        assert_eq!(txt2.to_string(&t2).as_str(), "hello world");

        txt1.insert(&mut t1, 5, " beautiful");
        txt1.insert(&mut t1, 21, "!");
        txt1.delete(&mut t1, 0, 5);
        assert_eq!(txt1.to_string(&t1).as_str(), " beautiful world!");

        txt2.delete(&mut t2, 5, 5);
        txt2.delete(&mut t2, 0, 1);
        txt2.insert(&mut t2, 0, "H");
        assert_eq!(txt2.to_string(&t2).as_str(), "Hellod");

        let sv1 = d1.get_state_vector(&t1);
        let sv2 = d2.get_state_vector(&t2);
        let u1 = d1.encode_delta_as_update(&sv2, &t1);
        let u2 = d2.encode_delta_as_update(&sv1, &t2);

        d1.apply_update(&mut t1, u2.as_slice());
        d2.apply_update(&mut t2, u1.as_slice());

        let a = txt1.to_string(&t1);
        let b = txt2.to_string(&t2);

        assert_eq!(a, b);
        assert_eq!(a, "H beautifuld!".to_owned());
    }
}
