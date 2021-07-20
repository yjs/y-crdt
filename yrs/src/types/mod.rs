pub mod map;
pub mod text;

use crate::*;
pub use map::Map;
pub use text::Text;

use crate::block::BlockPtr;
use std::cell::Cell;
use std::collections::HashMap;
use std::hash::Hasher;

pub struct Array {
    ptr: types::TypePtr,
}

pub struct XmlElement {
    ptr: types::TypePtr,
}

pub struct XmlFragment {
    ptr: types::TypePtr,
}

pub struct XmlHook {
    ptr: types::TypePtr,
}

pub struct XmlText {
    ptr: types::TypePtr,
}

pub enum SharedType {
    Text(Text),
    Array(Array),
    Map(Map),
    XmlElement(XmlElement),
    XmlFragment(XmlFragment),
    XmlHook(XmlHook),
    XmlText(XmlText),
}

pub type TypeRefs = u8;

pub const TYPE_REFS_ARRAY: TypeRefs = 0;
pub const TYPE_REFS_MAP: TypeRefs = 1;
pub const TYPE_REFS_TEXT: TypeRefs = 2;
pub const TYPE_REFS_XML_ELEMENT: TypeRefs = 3;
pub const TYPE_REFS_XML_FRAGMENT: TypeRefs = 4;
pub const TYPE_REFS_XML_HOOK: TypeRefs = 5;
pub const TYPE_REFS_XML_TEXT: TypeRefs = 6;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Inner {
    pub start: Cell<Option<BlockPtr>>,
    pub map: HashMap<String, BlockPtr>,
    pub ptr: TypePtr,
    pub name: Option<String>,
    pub type_ref: TypeRefs,
}

impl Inner {
    pub fn new(ptr: TypePtr, name: Option<String>, type_ref: TypeRefs) -> Self {
        Self {
            start: Cell::from(None),
            map: HashMap::default(),
            ptr,
            name,
            type_ref,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypePtr {
    NamedRef(u32),
    Id(block::BlockPtr),
    Named(String),
}

#[derive(Default)]
pub(crate) struct XorHasher(u64);

impl Hasher for XorHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut i = 0;
        let mut buf = [0u8; 8];
        while i <= bytes.len() - 8 {
            buf.copy_from_slice(&bytes[i..i + 8]);
            self.0 ^= u64::from_ne_bytes(buf);
            i += 8;
        }
        while i < bytes.len() {
            self.0 ^= bytes[i] as u64;
            i += 1;
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.0 ^= value as u64;
    }

    fn write_u64(&mut self, value: u64) {
        self.0 ^= value;
    }

    fn write_usize(&mut self, value: usize) {
        self.0 ^= value as u64;
    }
}
