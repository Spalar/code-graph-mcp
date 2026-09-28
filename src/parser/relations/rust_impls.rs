//! Which methods of a file's OTHER impl blocks a `self.m()` / `Self::m()` call
//! can never run (D#159).
//!
//! Impls of one type that differ only in their arguments
//! (`impl AsyncWrite for Cursor<&mut [u8]>`, `… for Cursor<Vec<u8>>`) all name
//! their type `Cursor` (`rust_impl_type_name`), so by type name a `self.m()` in
//! one reaches every one of them. Two facts of the language tell them apart
//! without knowing what the arguments mean:
//!
//! - Coherence: two impls of the SAME trait never cover one type. A
//!   `self.m()` in `impl W for A` runs `A`'s `W::m`, so the `m` of another
//!   `impl W for …` block is another type's. This holds through aliases too,
//!   since impls of one trait for alias-equal types would conflict.
//! - E0592: two inherent impls of one type may not both define `m`. When the
//!   caller's own inherent impl defines `m`, another inherent impl's `m` is
//!   another type's. When it does not, the other block may be the same type's
//!   split impl, so nothing is excluded.
//!
//! Impls of different traits, and impls in other files, stay candidates: the
//! rule only removes what the language rules out. The result is recorded on the
//! call as the lines of the excluded methods (`"xl"`), read back by
//! `resolve::self_filter_candidates` for candidates in the caller's own file,
//! which a re-index always re-parses together with the call.

use super::node_text;
use std::cell::RefCell;
use std::collections::HashMap;

struct ImplBlock {
    /// `rust_impl_type_name` of the impl's type.
    ty: String,
    /// The implemented trait as written, whitespace dropped (`From<u8>` and
    /// `From<u16>` are different traits); None for an inherent impl.
    of_trait: Option<String>,
    /// Methods defined directly in the block: name, 1-based start line (the
    /// line their node is stored with).
    methods: Vec<(String, u32)>,
}

impl ImplBlock {
    fn defines(&self, method: &str) -> bool {
        self.methods.iter().any(|(m, _)| m == method)
    }
}

thread_local! {
    /// The current file's impl blocks, keyed by the `impl_item` node id. Built on
    /// first use per file and cleared by [`reset`]: node ids are unique only
    /// within one tree.
    static BLOCKS: RefCell<Option<HashMap<usize, ImplBlock>>> = const { RefCell::new(None) };
}

/// Drop the current file's blocks. Called once per file before its walk.
pub(super) fn reset() {
    BLOCKS.with(|b| *b.borrow_mut() = None);
}

fn build(root: tree_sitter::Node, source: &str) -> HashMap<usize, ImplBlock> {
    let mut blocks = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "impl_item" {
            if let Some(ty) = node.child_by_field_name("type") {
                let of_trait = node.child_by_field_name("trait").map(|t| {
                    node_text(&t, source)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect()
                });
                let mut methods = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for item in body.named_children(&mut cursor) {
                        if item.kind() != "function_item" {
                            continue;
                        }
                        if let Some(name) = item.child_by_field_name("name") {
                            methods.push((
                                node_text(&name, source).to_string(),
                                item.start_position().row as u32 + 1,
                            ));
                        }
                    }
                }
                blocks.insert(
                    node.id(),
                    ImplBlock {
                        ty: crate::parser::rust_impl_type_name(node_text(&ty, source)),
                        of_trait,
                        methods,
                    },
                );
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    blocks
}

/// Whether the impl block around `call` is an inherent one (`impl Foo`, no
/// trait). Such an impl lives in its type's crate (E0116), and so does every
/// impl that crate can call on the type: another crate implementing a trait for
/// it would have to depend on this one. A `self`/`Self` call there never
/// reaches outside the crate.
pub(super) fn in_inherent_impl(call: tree_sitter::Node) -> bool {
    let mut cur = call.parent();
    while let Some(n) = cur {
        match n.kind() {
            "impl_item" => return n.child_by_field_name("trait").is_none(),
            "trait_item" => return false,
            _ => cur = n.parent(),
        }
    }
    false
}

/// What a `self`/`Self` call's own file says about its target.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SelfCallFacts {
    /// Start lines of the file's methods of that name the call cannot reach:
    /// other impls the language keeps apart from the caller's (module doc).
    pub excluded: Vec<u32>,
    /// The file defines the type's method of that name only in trait impls.
    /// An inherent method of that name outranks a trait's in method lookup,
    /// and one in another file of the crate may exist, so the file's own is no
    /// proof of the target (`resolve::self_filter_candidates`).
    pub trait_only_here: bool,
}

/// [`SelfCallFacts`] for a `self`/`Self` call at `call` of `method`. Empty
/// outside an impl block.
pub(super) fn self_call_facts(
    call: tree_sitter::Node,
    source: &str,
    method: &str,
) -> SelfCallFacts {
    let mut cur = call.parent();
    let mut caller_line = None;
    let own = loop {
        match cur {
            Some(n) if n.kind() == "impl_item" => break n,
            Some(n) if n.kind() == "trait_item" => return SelfCallFacts::default(),
            Some(n) => {
                if n.kind() == "function_item" && caller_line.is_none() {
                    caller_line = Some(n.start_position().row as u32 + 1);
                }
                cur = n.parent();
            }
            None => return SelfCallFacts::default(),
        }
    };
    BLOCKS.with(|cell| {
        if cell.borrow().is_none() {
            let mut root = call;
            while let Some(p) = root.parent() {
                root = p;
            }
            *cell.borrow_mut() = Some(build(root, source));
        }
        let blocks = cell.borrow();
        let blocks = blocks.as_ref().expect("built above");
        let Some(mine) = blocks.get(&own.id()) else {
            return SelfCallFacts::default();
        };
        let apart = mine.of_trait.is_some() || mine.defines(method);
        let mut excluded = Vec::new();
        let (mut inherent_here, mut trait_here) = (false, false);
        for (id, other) in blocks.iter().filter(|(_, b)| b.ty == mine.ty) {
            for (_, line) in other.methods.iter().filter(|(m, _)| m == method) {
                if Some(*line) == caller_line && *id == own.id() {
                    continue; // the caller itself says nothing about its callee
                }
                if apart && *id != own.id() && other.of_trait == mine.of_trait {
                    excluded.push(*line);
                } else if other.of_trait.is_none() {
                    inherent_here = true;
                } else {
                    trait_here = true;
                }
            }
        }
        excluded.sort_unstable();
        excluded.dedup();
        SelfCallFacts {
            excluded,
            trait_only_here: trait_here && !inherent_here,
        }
    })
}

/// `Pin`'s std methods that take `self` in some form.
const PIN_METHODS: &[&str] = &[
    "as_deref_mut",
    "as_mut",
    "as_ref",
    "get_mut",
    "get_ref",
    "get_unchecked_mut",
    "into_ref",
    "map_unchecked",
    "map_unchecked_mut",
    "set",
];

/// Whether a `self.method()` call is answered by the wrapper its enclosing
/// function declares `self` as, not by `Self`: with `self: Pin<&mut Self>`,
/// `self.get_mut()` is `Pin::get_mut`, and with `self: Arc<Self>`,
/// `self.clone()` is `Arc`'s — method lookup meets the receiver's own type
/// before any deref reaches `Self`. The wrapper is read by name, as the parser
/// reads these pointers everywhere (`rust_receiver.rs`).
pub(super) fn answered_by_the_self_wrapper(
    call: tree_sitter::Node,
    source: &str,
    method: &str,
) -> bool {
    let mut cur = call.parent();
    let function = loop {
        match cur {
            Some(n) if n.kind() == "function_item" => break n,
            Some(n) if matches!(n.kind(), "impl_item" | "trait_item" | "source_file") => {
                return false
            }
            Some(n) => cur = n.parent(),
            None => return false,
        }
    };
    let Some(params) = function.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = params.walk();
    let Some(receiver) = params
        .named_children(&mut cursor)
        .find(|p| p.kind() != "attribute_item")
    else {
        return false;
    };
    if receiver.kind() != "parameter"
        || receiver
            .child_by_field_name("pattern")
            .is_none_or(|p| p.kind() != "self")
    {
        return false;
    }
    let Some(ty) = receiver.child_by_field_name("type") else {
        return false;
    };
    if ty.kind() != "generic_type" {
        return false;
    }
    let Some(base) = ty.child_by_field_name("type") else {
        return false;
    };
    let text = node_text(&base, source);
    match text.rsplit("::").next().unwrap_or(text) {
        "Pin" => PIN_METHODS.contains(&method),
        "Arc" | "Rc" => method == "clone",
        _ => false,
    }
}
