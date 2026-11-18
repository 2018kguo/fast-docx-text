//! mammoth 1.13.0's `extractRawText`, element for element. A reimplementation of
//! mammoth's behaviour (Copyright (c) 2013, Michael Williamson, BSD-2-Clause; see
//! NOTICE).
//!
//! mammoth reads the package into its document model (`docx/docx-reader.js`,
//! `docx/body-reader.js`) and joins the text of that model, ending every paragraph with a
//! blank line (`raw-text.js`). This keeps the parts of the model that decide text: which
//! elements it reads at all (`w:del`, `w:moveFrom`, `w:fldSimple` and equations are
//! not read), paragraphs whose mark is deleted joining the next one,
//! VML text boxes placed after their paragraph, vertically merged table cells dropped,
//! and a checkbox content control's glyph replaced by the checkbox. It also keeps every
//! way mammoth fails on a package, so a file one reader refuses the other refuses too.

use std::collections::HashMap;

use crate::dingbats::DINGBATS;
use crate::tree::{Kind, NodeId, Tree};
use crate::{xmldom, zip};

/// Element names the reader matches, interned first so their ids are the constants
/// below.
const KNOWN: &[&str] = &[
    "w:p",
    "w:r",
    "w:hyperlink",
    "w:fldChar",
    "w:instrText",
    "w:t",
    "w:tab",
    "w:noBreakHyphen",
    "w:softHyphen",
    "w:sym",
    "w:tbl",
    "w:tr",
    "w:tc",
    "w:footnoteReference",
    "w:endnoteReference",
    "w:commentReference",
    "w:br",
    "w:bookmarkStart",
    "mc:AlternateContent",
    "w:sdt",
    "w:ins",
    "w:object",
    "w:smartTag",
    "w:drawing",
    "v:roundrect",
    "v:shape",
    "v:textbox",
    "w:txbxContent",
    "v:group",
    "v:rect",
    "w:pict",
    "wp:inline",
    "wp:anchor",
    "v:imagedata",
    "w:customXml",
    "w:moveTo",
];
const W_P: u32 = 0;
const W_R: u32 = 1;
const W_HYPERLINK: u32 = 2;
const W_FLDCHAR: u32 = 3;
const W_INSTRTEXT: u32 = 4;
const W_T: u32 = 5;
const W_TAB: u32 = 6;
const W_NOBREAKHYPHEN: u32 = 7;
const W_SOFTHYPHEN: u32 = 8;
const W_SYM: u32 = 9;
const W_TBL: u32 = 10;
const W_TR: u32 = 11;
const W_TC: u32 = 12;
const W_FOOTNOTEREFERENCE: u32 = 13;
const W_ENDNOTEREFERENCE: u32 = 14;
const W_COMMENTREFERENCE: u32 = 15;
const W_BR: u32 = 16;
const W_BOOKMARKSTART: u32 = 17;
const MC_ALTERNATECONTENT: u32 = 18;
const W_SDT: u32 = 19;
const W_INS: u32 = 20;
const W_OBJECT: u32 = 21;
const W_SMARTTAG: u32 = 22;
const W_DRAWING: u32 = 23;
const V_ROUNDRECT: u32 = 24;
const V_SHAPE: u32 = 25;
const V_TEXTBOX: u32 = 26;
const W_TXBXCONTENT: u32 = 27;
const V_GROUP: u32 = 28;
const V_RECT: u32 = 29;
const W_PICT: u32 = 30;
const WP_INLINE: u32 = 31;
const WP_ANCHOR: u32 = 32;
const V_IMAGEDATA: u32 = 33;
const W_CUSTOMXML: u32 = 34;
const W_MOVETO: u32 = 35;

enum El {
    /// Text: a range of the part's text, or a fixed string.
    Text(u32, u32),
    Str(std::borrow::Cow<'static, str>),
    Tab,
    Paragraph(Vec<El>),
    /// A run, hyperlink or other element whose text is its children's.
    Group(Vec<El>),
    Table(Vec<El>),
    Row(Vec<El>),
    Cell {
        children: Vec<El>,
        col_span: f64,
        v_merge: Option<bool>,
    },
    /// No text: a checkbox, break, image, note or comment reference, bookmark.
    Leaf,
}

/// What reading elements produces: their values, and the elements that surface after
/// the enclosing paragraph (a `w:pict`'s contents). Reads append to one.
#[derive(Default)]
struct Read {
    value: Vec<El>,
    extra: Vec<El>,
}

struct Relationships {
    by_id: HashMap<String, Option<String>>,
    by_type: HashMap<String, Vec<Option<String>>>,
}

impl Relationships {
    fn empty() -> Self {
        Relationships {
            by_id: HashMap::new(),
            by_type: HashMap::new(),
        }
    }
}

/// What `w:numPr` lookups can throw on: mammoth resolves a numbering style link through
/// the styles part and fails when the style is missing.
#[derive(Default)]
struct Numbering {
    /// numId → abstractNumId.
    nums: HashMap<String, String>,
    /// abstractNumId → numStyleLink.
    abstract_links: HashMap<String, Option<String>>,
    /// Numbering style id → numId.
    numbering_styles: HashMap<String, String>,
    /// Paragraph styles some numbering level names (`findLevelByParagraphStyleId`).
    level_styles: std::collections::HashSet<String>,
}

/// A JavaScript property key: a missing attribute is `undefined`.
fn key(value: Option<&str>) -> String {
    value.unwrap_or("undefined").to_owned()
}

impl Numbering {
    /// `findLevel`: follows numStyleLink through numbering styles, stopping at a numId
    /// it has already seen.
    fn find_level(&self, num_id: &str) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        let mut num_id = num_id;
        loop {
            if !seen.insert(num_id) {
                return Ok(());
            }
            let Some(abstract_id) = self.nums.get(num_id) else {
                return Ok(());
            };
            match self.abstract_links.get(abstract_id) {
                Some(Some(link)) => match self.numbering_styles.get(link) {
                    Some(style_num_id) => num_id = style_num_id,
                    None => {
                        return Err("Cannot read properties of undefined (reading 'numId')".into());
                    }
                },
                _ => return Ok(()),
            }
        }
    }
}

struct Part {
    tree: Tree,
    root: NodeId,
}

/// mammoth's `readXmlFromZipFile`: the part parsed and its root's `mc:AlternateContent`
/// collapsed, or None when the package has no such part.
fn read_part(package: &zip::Package, path: &str) -> Result<Option<Part>, String> {
    let Some(entry) = package.file(path) else {
        return Ok(None);
    };
    let source = xmldom::decode(entry.bytes()?);
    let mut tree = xmldom::parse(source, KNOWN)?;
    let root = tree
        .top_nodes()
        .iter()
        .copied()
        .find(|&node| matches!(tree.nodes[node as usize], Kind::Element { .. }))
        .ok_or("Cannot read properties of null (reading 'tagName')")?;
    if tree.name(root) == Some("mc:AlternateContent") {
        // The root's first Fallback child stands in for it, as it is (uncollapsed);
        // none, or a text node, fails wherever the part is read.
        let root = tree
            .first(root, "mc:Fallback")
            .and_then(|fallback| tree.kids(fallback).first().copied())
            .filter(|&node| matches!(tree.nodes[node as usize], Kind::Element { .. }))
            .ok_or("Cannot read properties of undefined (reading 'children')")?;
        return Ok(Some(Part { tree, root }));
    }
    collapse_alternate_content(&mut tree, root);
    Ok(Some(Part { tree, root }))
}

/// `collapseAlternateContent` from the root: each `mc:AlternateContent` child becomes the
/// children of its first `mc:Fallback`. Those children are taken as they are: the
/// collapse does not continue below them.
fn collapse_alternate_content(tree: &mut Tree, root: NodeId) {
    let Some(alternate) = tree.id("mc:AlternateContent") else {
        return;
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let kids = tree.kids(node).to_vec();
        if !kids.iter().any(|&kid| tree.name_id(kid) == Some(alternate)) {
            stack.extend(kids.iter().filter(|&&kid| tree.name_id(kid).is_some()));
            continue;
        }
        let mut collapsed = Vec::with_capacity(kids.len());
        for kid in kids {
            if tree.name_id(kid) == Some(alternate) {
                if let Some(fallback) = tree.first(kid, "mc:Fallback") {
                    collapsed.extend_from_slice(tree.kids(fallback));
                }
            } else {
                if tree.name_id(kid).is_some() {
                    stack.push(kid);
                }
                collapsed.push(kid);
            }
        }
        let start = tree.children.len() as u32;
        tree.children.extend(collapsed);
        let end = tree.children.len() as u32;
        if let Kind::Element { children, .. } = &mut tree.nodes[node as usize] {
            *children = (start, end);
        }
    }
}

fn read_relationships(part: Option<&Part>) -> Relationships {
    let mut relationships = Relationships::empty();
    let Some(part) = part else {
        return relationships;
    };
    let tree = &part.tree;
    for &child in tree.kids(part.root) {
        if tree.name(child) != Some("relationships:Relationship") {
            continue;
        }
        let id = key(tree.attr(child, "Id"));
        let target = tree.attr(child, "Target").map(str::to_owned);
        let kind = key(tree.attr(child, "Type"));
        relationships.by_id.insert(id, target.clone());
        relationships.by_type.entry(kind).or_default().push(target);
    }
    relationships
}

fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => ("", path),
    }
}

/// mammoth's `joinPath`: non-empty segments joined with `/`, restarting at an absolute one.
fn join_path(parts: &[&str]) -> String {
    let mut relevant: Vec<&str> = Vec::new();
    for part in parts.iter().filter(|part| !part.is_empty()) {
        if part.starts_with('/') {
            relevant = vec![part];
        } else {
            relevant.push(part);
        }
    }
    relevant.join("/")
}

fn relationships_path(path: &str) -> String {
    let (dir, base) = split_path(path);
    join_path(&[dir, "_rels", &format!("{base}.rels")])
}

fn find_part_path(
    package: &zip::Package,
    relationships: &Relationships,
    kind: &str,
    base: &str,
    fallback: &str,
) -> String {
    relationships
        .by_type
        .get(kind)
        .into_iter()
        .flatten()
        .map(|target| {
            let joined = join_path(&[base, target.as_deref().unwrap_or("")]);
            joined
                .strip_prefix('/')
                .map(str::to_owned)
                .unwrap_or(joined)
        })
        .find(|target| package.file(target).is_some())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Parses the part's relationships and the part, as `readXmlFileWithBody` does.
fn read_part_with_relationships(
    package: &zip::Package,
    path: &str,
) -> Result<(Relationships, Option<Part>), String> {
    let relationships = read_relationships(read_part(package, &relationships_path(path))?.as_ref());
    Ok((relationships, read_part(package, path)?))
}

fn read_numbering(numbering: Option<&Part>, styles: Option<&Part>) -> Result<Numbering, String> {
    let mut result = Numbering::default();
    if let Some(Part { tree, root }) = styles {
        for &style in tree.kids(*root) {
            if tree.name(style) != Some("w:style")
                || tree.attr(style, "w:type") != Some("numbering")
            {
                continue;
            }
            let style_id = key(tree.attr(style, "w:styleId"));
            let num_id = key(tree
                .first(style, "w:pPr")
                .and_then(|ppr| tree.first(ppr, "w:numPr"))
                .and_then(|numpr| tree.first(numpr, "w:numId"))
                .and_then(|numid| tree.attr(numid, "w:val")));
            result.numbering_styles.entry(style_id).or_insert(num_id);
        }
    }
    if let Some(Part { tree, root }) = numbering {
        for &child in tree.kids(*root) {
            match tree.name(child) {
                Some("w:abstractNum") => {
                    // readAbstractNum: levels by w:ilvl (a later one replaces), then a
                    // level without an index as level "0" when none is.
                    let mut levels: HashMap<String, Option<String>> = HashMap::new();
                    let mut without_index = None;
                    for &level in tree.kids(child) {
                        if tree.name(level) != Some("w:lvl") {
                            continue;
                        }
                        let style = tree
                            .first(level, "w:pStyle")
                            .and_then(|style| tree.attr(style, "w:val"))
                            .map(str::to_owned);
                        match tree.attr(level, "w:ilvl") {
                            Some(index) => {
                                levels.insert(index.to_owned(), style);
                            }
                            None => without_index = Some(style),
                        }
                    }
                    if let Some(style) = without_index {
                        levels.entry("0".into()).or_insert(style);
                    }
                    result.level_styles.extend(levels.into_values().flatten());
                    let link = tree
                        .first(child, "w:numStyleLink")
                        .and_then(|link| tree.attr(link, "w:val"))
                        .map(str::to_owned);
                    result
                        .abstract_links
                        .insert(key(tree.attr(child, "w:abstractNumId")), link);
                }
                Some("w:num") => {
                    let abstract_id = tree
                        .first(child, "w:abstractNumId")
                        .ok_or("Cannot read properties of undefined (reading 'attributes')")?;
                    result.nums.insert(
                        key(tree.attr(child, "w:numId")),
                        key(tree.attr(abstract_id, "w:val")),
                    );
                }
                _ => {}
            }
        }
    }
    Ok(result)
}

/// The parts [`extract`] finds through relationships, which need not be named .xml:
/// the main document and its styles, numbering, notes and comments, resolved as
/// `extract` resolves them. None when a relationship part it would read declares
/// more than `max_rels` bytes, so the caller's parsing stays small.
pub fn related_parts(package: &zip::Package, max_rels: u64) -> Option<Vec<String>> {
    let relationships = |path: &str| {
        if package
            .file(path)
            .is_some_and(|entry| entry.declared_size() > max_rels)
        {
            return None;
        }
        Some(read_relationships(
            read_part(package, path).ok().flatten().as_ref(),
        ))
    };
    let main = find_part_path(
        package,
        &relationships("_rels/.rels")?,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
        "",
        "word/document.xml",
    );
    let document_relationships = relationships(&relationships_path(&main))?;
    let (main_dir, _) = split_path(&main);
    let mut parts: Vec<String> = ["styles", "numbering", "footnotes", "endnotes", "comments"]
        .iter()
        .map(|name| {
            find_part_path(
                package,
                &document_relationships,
                &format!(
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/{name}"
                ),
                main_dir,
                &format!("word/{name}.xml"),
            )
        })
        .collect();
    parts.push(main);
    Some(parts)
}

pub fn extract(data: &[u8]) -> Result<String, String> {
    let package = zip::open(data)?;
    read_part(&package, "[Content_Types].xml")?;
    let package_relationships = read_relationships(read_part(&package, "_rels/.rels")?.as_ref());
    let main = find_part_path(
        &package,
        &package_relationships,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
        "",
        "word/document.xml",
    );
    if package.file(&main).is_none() {
        return Err(
            "Could not find main document part. Are you sure this is a valid .docx file?".into(),
        );
    }
    let document_relationships =
        read_relationships(read_part(&package, &relationships_path(&main))?.as_ref());
    let (main_dir, _) = split_path(&main);
    let related = |name: &str| {
        find_part_path(
            &package,
            &document_relationships,
            &format!("http://schemas.openxmlformats.org/officeDocument/2006/relationships/{name}"),
            main_dir,
            &format!("word/{name}.xml"),
        )
    };
    let styles = read_part(&package, &related("styles"))?;
    let numbering_part = read_part(&package, &related("numbering"))?;
    let numbering = read_numbering(numbering_part.as_ref(), styles.as_ref())?;

    // Notes and comments are not in the raw text, but mammoth reads them, and fails
    // when they fail.
    for (name, element) in [
        ("footnotes", "w:footnote"),
        ("endnotes", "w:endnote"),
        ("comments", "w:comment"),
    ] {
        let (relationships, part) = read_part_with_relationships(&package, &related(name))?;
        if let Some(Part { tree, root }) = &part {
            for &note in tree.kids(*root) {
                if tree.name(note) != Some(element) {
                    continue;
                }
                if element != "w:comment"
                    && matches!(
                        tree.attr(note, "w:type"),
                        Some("continuationSeparator" | "separator")
                    )
                {
                    continue;
                }
                Body::new(tree, &relationships, &numbering).read_all(tree.kids(note))?;
            }
        }
    }

    let (relationships, part) = read_part_with_relationships(&package, &main)?;
    let Part { tree, root } = part.expect("the main part exists");
    let body = tree
        .first(root, "w:body")
        .ok_or("Could not find the body element: are you sure this is a docx file?")?;
    let read = Body::new(&tree, &relationships, &numbering).read_all(tree.kids(body))?;
    // Sized first, so a long text is never copied into a doubled buffer.
    let len = read
        .value
        .iter()
        .map(|element| text_len(&tree, element))
        .sum();
    let mut out = String::with_capacity(len);
    for element in &read.value {
        write_text(&tree, element, &mut out);
    }
    debug_assert_eq!(out.len(), len);
    Ok(out)
}

/// The byte length [`write_text`] writes.
fn text_len(tree: &Tree, element: &El) -> usize {
    let children = |children: &[El]| {
        children
            .iter()
            .map(|child| text_len(tree, child))
            .sum::<usize>()
    };
    match element {
        El::Text(start, end) => tree.str((*start, *end)).len(),
        El::Str(text) => text.len(),
        El::Tab => 1,
        El::Paragraph(items) => children(items) + 2,
        El::Group(items)
        | El::Table(items)
        | El::Row(items)
        | El::Cell {
            children: items, ..
        } => children(items),
        El::Leaf => 0,
    }
}

fn write_text(tree: &Tree, element: &El, out: &mut String) {
    match element {
        El::Text(start, end) => out.push_str(tree.str((*start, *end))),
        El::Str(text) => out.push_str(text),
        El::Tab => out.push('\t'),
        El::Paragraph(children) => {
            children
                .iter()
                .for_each(|child| write_text(tree, child, out));
            out.push_str("\n\n");
        }
        El::Group(children) | El::Table(children) | El::Row(children) => {
            children
                .iter()
                .for_each(|child| write_text(tree, child, out));
        }
        El::Cell { children, .. } => children
            .iter()
            .for_each(|child| write_text(tree, child, out)),
        El::Leaf => {}
    }
}

/// mammoth's `BodyReader`: one per `readXmlElements` call, holding the open complex
/// fields and the contents of paragraphs whose mark was deleted.
struct Body<'a> {
    tree: &'a Tree,
    relationships: &'a Relationships,
    numbering: &'a Numbering,
    open_fields: usize,
    deleted_paragraph_contents: Vec<NodeId>,
}

impl<'a> Body<'a> {
    fn new(tree: &'a Tree, relationships: &'a Relationships, numbering: &'a Numbering) -> Self {
        Body {
            tree,
            relationships,
            numbering,
            open_fields: 0,
            deleted_paragraph_contents: Vec::new(),
        }
    }

    fn read_all(&mut self, nodes: &[NodeId]) -> Result<Read, String> {
        let mut read = Read::default();
        self.read_into(nodes, &mut read)?;
        Ok(read)
    }

    fn read_into(&mut self, nodes: &[NodeId], out: &mut Read) -> Result<(), String> {
        for &node in nodes {
            self.read(node, out)?;
        }
        Ok(())
    }

    fn children(&mut self, node: NodeId) -> Result<Read, String> {
        self.read_all(self.tree.kids(node))
    }

    /// mammoth's `Element.text()`: one text child, or none.
    fn text(&self, node: NodeId) -> Result<El, String> {
        match self.tree.kids(node) {
            [] => Ok(El::Text(0, 0)),
            [only] => match self.tree.nodes[*only as usize] {
                Kind::Text(start, end) => Ok(El::Text(start, end)),
                Kind::Element { .. } => Err("Not implemented".into()),
            },
            _ => Err("Not implemented".into()),
        }
    }

    fn first(&self, node: Option<NodeId>, name: &str) -> Option<NodeId> {
        node.and_then(|node| self.tree.first(node, name))
    }

    fn attr(&self, node: Option<NodeId>, name: &str) -> Option<&'a str> {
        node.and_then(|node| self.tree.attr(node, name))
    }

    /// Reads one node into `out`.
    fn read(&mut self, node: NodeId, out: &mut Read) -> Result<(), String> {
        let tree = self.tree;
        let Some(name) = tree.name_id(node) else {
            return Ok(());
        };
        match name {
            W_P => {
                let properties = tree.first(node, "w:pPr");
                let deleted = self
                    .first(self.first(properties, "w:rPr"), "w:del")
                    .is_some();
                if deleted {
                    self.deleted_paragraph_contents
                        .extend_from_slice(tree.kids(node));
                    return Ok(());
                }
                self.paragraph_properties(properties)?;
                let read = if self.deleted_paragraph_contents.is_empty() {
                    self.read_all(tree.kids(node))?
                } else {
                    let mut nodes = std::mem::take(&mut self.deleted_paragraph_contents);
                    nodes.extend_from_slice(tree.kids(node));
                    self.read_all(&nodes)?
                };
                out.value.push(El::Paragraph(read.value));
                out.value.extend(read.extra);
            }
            W_R | W_HYPERLINK => {
                if name == W_HYPERLINK {
                    let relationship = tree.attr(node, "r:id");
                    let anchor = tree.attr(node, "w:anchor");
                    if let (Some(id), Some(_)) = (
                        relationship.filter(|id| !id.is_empty()),
                        anchor.filter(|a| !a.is_empty()),
                    ) && self.target(id).is_none()
                    {
                        return Err(
                            "Cannot read properties of undefined (reading 'indexOf')".into()
                        );
                    }
                }
                let Read { value, extra } = self.children(node)?;
                out.value.push(El::Group(value));
                out.extra.extend(extra);
            }
            W_FLDCHAR => {
                match tree.attr(node, "w:fldCharType") {
                    Some("begin") => self.open_fields += 1,
                    // An end or separator with no field open is ignored, with a warning.
                    Some("end" | "separate") if self.open_fields == 0 => {
                        return Ok(());
                    }
                    Some("end") => self.open_fields -= 1,
                    _ => {}
                }
                out.value.push(El::Leaf);
            }
            W_INSTRTEXT => {
                self.text(node)?;
            }
            W_T => out.value.push(self.text(node)?),
            W_TAB => out.value.push(El::Tab),
            W_NOBREAKHYPHEN => out.value.push(El::Str("\u{2011}".into())),
            W_SOFTHYPHEN => out.value.push(El::Str("\u{00AD}".into())),
            W_SYM => {
                if let Some(text) = self.symbol(node)? {
                    out.value.push(El::Str(text.into()));
                }
            }
            W_TBL => {
                let Read { value, extra } = self.children(node)?;
                out.value.push(El::Table(merge_row_spans(value)));
                out.extra.extend(extra);
            }
            W_TR => {
                if self.first(tree.first(node, "w:trPr"), "w:del").is_some() {
                    return Ok(());
                }
                let Read { value, extra } = self.children(node)?;
                out.value.push(El::Row(value));
                out.extra.extend(extra);
            }
            W_TC => {
                let Read { value, extra } = self.children(node)?;
                let properties = tree.first(node, "w:tcPr");
                let grid_span = self.attr(self.first(properties, "w:gridSpan"), "w:val");
                let col_span = match grid_span {
                    Some(span) if !span.is_empty() => js_parse_int(span),
                    _ => 1.0,
                };
                let v_merge = self.first(properties, "w:vMerge").map(|merge| {
                    match tree.attr(merge, "w:val") {
                        Some(value) if !value.is_empty() => value == "continue",
                        _ => true,
                    }
                });
                out.value.push(El::Cell {
                    children: value,
                    col_span,
                    v_merge,
                });
                out.extra.extend(extra);
            }
            W_FOOTNOTEREFERENCE | W_ENDNOTEREFERENCE | W_COMMENTREFERENCE | W_BR => {
                out.value.push(El::Leaf);
            }
            W_BOOKMARKSTART => {
                if tree.attr(node, "w:name") != Some("_GoBack") {
                    out.value.push(El::Leaf);
                }
            }
            MC_ALTERNATECONTENT => {
                if let Some(fallback) = tree.first(node, "mc:Fallback") {
                    self.read_into(tree.kids(fallback), out)?;
                }
            }
            W_SDT => {
                let content = tree.first(node, "w:sdtContent");
                let Read { mut value, extra } = match content {
                    Some(content) => self.children(content)?,
                    None => Read::default(),
                };
                let checkbox = self.first(tree.first(node, "w:sdtPr"), "wordml:checkbox");
                if checkbox.is_some() {
                    let mut replaced = false;
                    for element in &mut value {
                        replace_first_text(element, &mut replaced);
                    }
                    if !replaced {
                        value = vec![El::Leaf];
                    }
                }
                out.value.extend(value);
                out.extra.extend(extra);
            }
            W_INS | W_OBJECT | W_SMARTTAG | W_DRAWING | V_ROUNDRECT | V_SHAPE | V_TEXTBOX
            | W_TXBXCONTENT | V_GROUP | V_RECT | W_CUSTOMXML | W_MOVETO => {
                self.read_into(tree.kids(node), out)?;
            }
            W_PICT => {
                let Read { value, extra } = self.children(node)?;
                out.extra.extend(extra);
                out.extra.extend(value);
            }
            WP_INLINE | WP_ANCHOR => {
                self.drawing(node)?;
            }
            V_IMAGEDATA => {
                if let Some(id) = tree.attr(node, "r:id").filter(|id| !id.is_empty()) {
                    self.embedded_image(id)?;
                }
            }
            // Everything else, including the elements mammoth ignores by name.
            _ => {}
        }
        Ok(())
    }

    fn target(&self, id: &str) -> Option<&'a str> {
        self.relationships
            .by_id
            .get(id)
            .and_then(|target| target.as_deref())
    }

    /// `findEmbeddedImageFile` fails on a relationship with no target.
    fn embedded_image(&self, id: &str) -> Result<(), String> {
        match self.target(id) {
            Some(_) => Ok(()),
            None => Err("Cannot read properties of undefined (reading 'charAt')".into()),
        }
    }

    fn drawing(&self, node: NodeId) -> Result<(), String> {
        let tree = self.tree;
        let mut level = vec![node];
        for name in [
            "a:graphic",
            "a:graphicData",
            "pic:pic",
            "pic:blipFill",
            "a:blip",
        ] {
            level = level
                .iter()
                .flat_map(|&parent| {
                    tree.kids(parent)
                        .iter()
                        .copied()
                        .filter(|&child| tree.name(child) == Some(name))
                })
                .collect();
        }
        for blip in level {
            if let Some(id) = tree.attr(blip, "r:embed").filter(|id| !id.is_empty()) {
                self.embedded_image(id)?;
            } else if let Some(id) = tree.attr(blip, "r:link").filter(|id| !id.is_empty())
                && self.target(id).is_none()
            {
                return Err("Cannot read properties of undefined (reading 'split')".into());
            }
        }
        Ok(())
    }

    /// The numbering lookups `readParagraphProperties` makes, for the ones that throw.
    fn paragraph_properties(&self, properties: Option<NodeId>) -> Result<(), String> {
        let numbering = self.first(properties, "w:numPr");
        let level = self.attr(self.first(numbering, "w:ilvl"), "w:val");
        let num_id = self.attr(self.first(numbering, "w:numId"), "w:val");
        let style = self.attr(self.first(properties, "w:pStyle"), "w:val");
        match (level, num_id) {
            (Some(_), Some(num_id)) => self.numbering.find_level(num_id),
            (_, Some(num_id))
                if !style.is_some_and(|style| self.numbering.level_styles.contains(style)) =>
            {
                self.numbering.find_level(num_id)
            }
            _ => Ok(()),
        }
    }

    fn symbol(&self, node: NodeId) -> Result<Option<String>, String> {
        let font = self
            .tree
            .attr(node, "w:font")
            .ok_or("Cannot read properties of undefined (reading 'toUpperCase')")?;
        let char_code = self.tree.attr(node, "w:char");
        let lookup = |hex: Option<&str>| -> Option<String> {
            let code = js_parse_int_radix(hex.unwrap_or("undefined"), 16);
            if code.is_nan() {
                return None;
            }
            // dingbats[TYPEFACE][code]: the table's keys are the integers 0-255.
            let typeface = font.to_uppercase();
            let (_, codes) = DINGBATS.iter().find(|(name, _)| *name == typeface)?;
            (code >= 0.0 && code < 256.0 && code.fract() == 0.0)
                .then(|| codes[code as usize])
                .filter(|&unicode| unicode != 0)
                .and_then(char::from_u32)
                .map(String::from)
        };
        Ok(lookup(char_code).or_else(|| {
            let code = char_code?;
            (code.chars().count() == 4 && code.starts_with("F0"))
                .then(|| lookup(Some(&code[2..])))
                .flatten()
        }))
    }
}

fn replace_first_text(element: &mut El, replaced: &mut bool) {
    match element {
        El::Text(start, end) if start != end && !*replaced => {
            *replaced = true;
            *element = El::Leaf;
        }
        El::Str(text) if !text.is_empty() && !*replaced => {
            *replaced = true;
            *element = El::Leaf;
        }
        El::Paragraph(children)
        | El::Group(children)
        | El::Table(children)
        | El::Row(children)
        | El::Cell { children, .. } => {
            for child in children {
                replace_first_text(child, replaced);
            }
        }
        _ => {}
    }
}

/// `calculateRowSpans`: when the table holds only rows of cells, a cell continuing a
/// vertical merge is dropped (with its text).
fn merge_row_spans(mut rows: Vec<El>) -> Vec<El> {
    let all_cells = rows.iter().all(|row| match row {
        El::Row(cells) => cells.iter().all(|cell| matches!(cell, El::Cell { .. })),
        _ => false,
    });
    if !all_cells {
        return rows;
    }
    // Column index → whether a cell starts a merge there. Indexes are JavaScript
    // numbers: NaN after an unparseable gridSpan, and every NaN is one key.
    let mut columns: Vec<f64> = Vec::new();
    let same = |a: f64, b: f64| a == b || (a.is_nan() && b.is_nan());
    for row in &mut rows {
        let El::Row(cells) = row else { unreachable!() };
        let mut index = 0f64;
        let mut keep = Vec::with_capacity(cells.len());
        for cell in cells.drain(..) {
            let El::Cell {
                col_span, v_merge, ..
            } = &cell
            else {
                unreachable!()
            };
            let col_span = *col_span;
            let continues = v_merge.unwrap_or(false) && columns.iter().any(|&c| same(c, index));
            if !continues {
                if !columns.iter().any(|&c| same(c, index)) {
                    columns.push(index);
                }
                keep.push(cell);
            }
            index += col_span;
        }
        *cells = keep;
    }
    rows
}

/// `parseInt(value, 10)` as a JavaScript number (NaN when no digits lead).
fn js_parse_int(value: &str) -> f64 {
    js_parse_int_radix(value, 10)
}

fn js_parse_int_radix(value: &str, radix: u32) -> f64 {
    let trimmed = value.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    let (negative, rest) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let rest = if radix == 16 {
        rest.strip_prefix("0x")
            .or_else(|| rest.strip_prefix("0X"))
            .unwrap_or(rest)
    } else {
        rest
    };
    let digits: Vec<u32> = rest.chars().map_while(|c| c.to_digit(radix)).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    let value = digits.iter().fold(0f64, |acc, &digit| {
        acc * f64::from(radix) + f64::from(digit)
    });
    if negative { -value } else { value }
}
