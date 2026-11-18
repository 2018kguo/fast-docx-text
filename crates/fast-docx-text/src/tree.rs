//! The parsed part both tokenizers build: elements and text in one arena, each element's
//! children and attributes as contiguous runs, names interned.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

pub type NodeId = u32;

/// FxHash (rustc's): SipHash on short element names was a tenth of the parse.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().unwrap());
            self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(FX_SEED);
        }
        let tail = chunks
            .remainder()
            .iter()
            .fold(0u64, |word, &byte| (word << 8) | u64::from(byte));
        self.0 = (self.0.rotate_left(5) ^ tail).wrapping_mul(FX_SEED);
    }
    fn write_u8(&mut self, byte: u8) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(byte)).wrapping_mul(FX_SEED);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

#[derive(Clone, Copy)]
pub enum Kind {
    /// An element: its interned name, and its children and attributes as runs of
    /// `Tree::children` and `Tree::attrs`.
    Element {
        name: u32,
        children: (u32, u32),
        attrs: (u32, u32),
    },
    /// Text: a range (see [`Tree::str`]).
    Text(u32, u32),
}

/// Marks a range of `Tree::text` rather than of `Tree::source`.
const DECODED: u32 = 1 << 31;

pub struct Tree {
    pub nodes: Vec<Kind>,
    pub children: Vec<NodeId>,
    /// (interned name, value range).
    pub attrs: Vec<(u32, (u32, u32))>,
    /// The part as parsed: text and values that needed no decoding are ranges of it.
    pub source: String,
    /// Text and values that differ from their source (entities decoded).
    pub text: String,
    names: Vec<String>,
    lookup: FxMap<String, u32>,
    /// The top-level nodes, in document order.
    pub top: (u32, u32),
}

impl Default for Tree {
    fn default() -> Self {
        Tree {
            nodes: Vec::new(),
            children: Vec::new(),
            attrs: Vec::new(),
            source: String::new(),
            text: String::new(),
            names: Vec::new(),
            lookup: FxMap::default(),
            top: (0, 0),
        }
    }
}

impl Tree {
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.lookup.get(name) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_owned());
        self.lookup.insert(name.to_owned(), id);
        id
    }

    /// The interned id of `name`, if any node or attribute uses it.
    pub fn id(&self, name: &str) -> Option<u32> {
        self.lookup.get(name).copied()
    }

    pub fn push_text(&mut self, text: &str) -> (u32, u32) {
        let start = self.text.len() as u32;
        self.text.push_str(text);
        (start | DECODED, self.text.len() as u32 | DECODED)
    }

    /// A text or attribute range: of `source`, or of `text` when marked [`DECODED`].
    pub fn str(&self, range: (u32, u32)) -> &str {
        if range.0 & DECODED != 0 {
            &self.text[(range.0 & !DECODED) as usize..(range.1 & !DECODED) as usize]
        } else {
            &self.source[range.0 as usize..range.1 as usize]
        }
    }

    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    pub fn name_of(&self, id: u32) -> &str {
        &self.names[id as usize]
    }

    /// An element's name, or None for text.
    pub fn name(&self, node: NodeId) -> Option<&str> {
        match self.nodes[node as usize] {
            Kind::Element { name, .. } => Some(&self.names[name as usize]),
            Kind::Text(..) => None,
        }
    }

    pub fn name_id(&self, node: NodeId) -> Option<u32> {
        match self.nodes[node as usize] {
            Kind::Element { name, .. } => Some(name),
            Kind::Text(..) => None,
        }
    }

    pub fn text_of(&self, node: NodeId) -> Option<&str> {
        match self.nodes[node as usize] {
            Kind::Text(start, end) => Some(self.str((start, end))),
            Kind::Element { .. } => None,
        }
    }

    pub fn kids(&self, node: NodeId) -> &[NodeId] {
        match self.nodes[node as usize] {
            Kind::Element {
                children: (start, end),
                ..
            } => &self.children[start as usize..end as usize],
            Kind::Text(..) => &[],
        }
    }

    pub fn top_nodes(&self) -> &[NodeId] {
        &self.children[self.top.0 as usize..self.top.1 as usize]
    }

    pub fn attr_list(&self, node: NodeId) -> &[(u32, (u32, u32))] {
        match self.nodes[node as usize] {
            Kind::Element {
                attrs: (start, end),
                ..
            } => &self.attrs[start as usize..end as usize],
            Kind::Text(..) => &[],
        }
    }

    /// The value of the attribute named exactly `name` (the last one, if repeated).
    pub fn attr(&self, node: NodeId, name: &str) -> Option<&str> {
        self.attr_id(node, self.id(name)?)
    }

    pub fn attr_id(&self, node: NodeId, id: u32) -> Option<&str> {
        self.attr_list(node)
            .iter()
            .rev()
            .find(|(attr, _)| *attr == id)
            .map(|(_, value)| self.str(*value))
    }

    /// The first child element named exactly `name`.
    pub fn first(&self, node: NodeId, name: &str) -> Option<NodeId> {
        self.first_id(node, self.id(name)?)
    }

    pub fn first_id(&self, node: NodeId, id: u32) -> Option<NodeId> {
        self.kids(node)
            .iter()
            .copied()
            .find(|&child| self.name_id(child) == Some(id))
    }
}

/// Builds a tree the way both parsers do: the children of every open element in one
/// buffer, each element's run copied out when it closes.
pub struct Builder {
    pub tree: Tree,
    buffer: Vec<NodeId>,
    open: Vec<(u32, (u32, u32), usize)>,
}

impl Builder {
    /// A builder whose tree has `known[i]` interned as id `i`, so a reader can match
    /// names as constants.
    pub fn new(known: &[&str]) -> Self {
        let mut tree = Tree::default();
        for name in known {
            tree.intern(name);
        }
        Builder {
            tree,
            buffer: Vec::new(),
            open: Vec::new(),
        }
    }

    pub fn depth(&self) -> usize {
        self.open.len()
    }

    /// A text node holding decoded `text`.
    pub fn text(&mut self, text: &str) {
        let range = self.tree.push_text(text);
        self.text_range(range);
    }

    /// A text node that is the source's bytes `start..end` as they are.
    pub fn text_range(&mut self, range: (u32, u32)) {
        let id = self.tree.nodes.len() as NodeId;
        self.tree.nodes.push(Kind::Text(range.0, range.1));
        self.buffer.push(id);
    }

    /// Starts an element whose attributes are the last `attr_count` pushed by `attr`.
    pub fn open(&mut self, name: u32, attr_start: usize) {
        let attrs = (attr_start as u32, self.tree.attrs.len() as u32);
        self.open.push((name, attrs, self.buffer.len()));
    }

    /// An attribute holding decoded `value`.
    pub fn attr(&mut self, name: u32, value: &str) {
        let range = self.tree.push_text(value);
        self.tree.attrs.push((name, range));
    }

    /// An attribute whose value is the source's bytes `start..end` as they are.
    pub fn attr_range(&mut self, name: u32, range: (u32, u32)) {
        self.tree.attrs.push((name, range));
    }

    pub fn attr_mark(&self) -> usize {
        self.tree.attrs.len()
    }

    pub fn open_name(&self) -> Option<u32> {
        self.open.last().map(|(name, _, _)| *name)
    }

    pub fn close(&mut self) {
        let Some((name, attrs, start)) = self.open.pop() else {
            return;
        };
        let child_start = self.tree.children.len() as u32;
        self.tree.children.extend_from_slice(&self.buffer[start..]);
        self.buffer.truncate(start);
        let id = self.tree.nodes.len() as NodeId;
        self.tree.nodes.push(Kind::Element {
            name,
            children: (child_start, self.tree.children.len() as u32),
            attrs,
        });
        self.buffer.push(id);
    }

    /// The tree, with open elements closed, holding the `source` its ranges point into.
    pub fn finish(mut self, source: String) -> Tree {
        while !self.open.is_empty() {
            self.close();
        }
        let start = self.tree.children.len() as u32;
        self.tree.children.extend_from_slice(&self.buffer);
        self.tree.top = (start, self.tree.children.len() as u32);
        self.tree.source = source;
        self.tree
    }
}
