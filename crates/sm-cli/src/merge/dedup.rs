//! Fallback rung 7b: refuse a clean merge that invented a duplicate
//! declaration.
//!
//! # The bug this catches
//!
//! `sm-merge`'s crate docs name it as a known limitation: *"two sides adding
//! methods with the same signature and different bodies merge to two
//! methods"*. The M5 corpus replay found it happening for real —
//! `alibaba/druid`'s `DataSourceHolder`, where one branch renamed the body of
//! `restart()` to call `resetState()` and the merge emitted **both**
//! `restart()` methods:
//!
//! ```text
//! public void restart() { dataSource.restart(); }
//! public void restart() { dataSource.resetState(); }
//! ```
//!
//! That does not compile, and — worse for us — it *parses*, so the reparse
//! check waves it through, and the tokens all came from real inputs, so the
//! token-authenticity check waves it through too. It is precisely the silently
//! wrong merge SPEC.md §0.4 rules out categorically.
//!
//! Both Spork and Mergiraf handle this with a **signature-keyed duplicate
//! post-pass** over the merged tree (docs/prior-art.md §8.2.4). We do the same
//! detection, but at a different place in the pipeline: here, as a rung of the
//! driver's existing fallback ladder, rather than as a transform inside
//! `sm-merge`. The reasoning is SPEC.md §0.4's — *a conflict is always an
//! acceptable answer, a wrong merge never is*. Turning the pair into a conflict
//! region inside the merged tree would be a better user experience and it means
//! touching the merged tree's child lists, gaps and layout invariants; falling
//! back to `git merge-file` costs a coarser conflict and cannot introduce a new
//! failure mode, because the line merge has already run and its bytes are
//! already in hand. The better version belongs in `sm-merge` and is recorded as
//! future work.
//!
//! # The rule
//!
//! A declaration's **key** is the chain of names of the declarations enclosing
//! it, plus its own kind, name and — for anything with a parameter list — the
//! whitespace-normalised text of that list. Count the keys in the output; count
//! them in each input. If the output holds *more* copies of a key than any
//! single input did, the merge manufactured a duplicate.
//!
//! Comparing against the inputs is what makes the rule safe rather than
//! opinionated: a file that already contained two same-named members (an
//! overload the parameter text does not distinguish, a name repeated in two
//! nested classes the key does not separate) is unchanged by the merge and
//! passes. Only an *increase* is reported.
//!
//! Two deliberate under-approximations, both in the direction of not firing:
//!
//! * **Parameter text, not erased types.** Java's rule is that two methods may
//!   not share an erasure, so `f(List<String>)` and `f(List<Integer>)` clash
//!   and this key does not see it. Computing erasures needs a type model we do
//!   not have and would be Java-specific; the parameter *text* is
//!   language-agnostic and every duplicate it does report is a real one.
//! * **Names only, no scope model.** Two classes in one file that each declare
//!   `foo()` are told apart by the enclosing-name chain, but a local class
//!   inside a method is not. Again: it can miss, it does not misfire.
//!
//! The check is language-agnostic — it asks the [`Language`] trait what a
//! declaration is and what it is called — and costs one walk of four trees the
//! driver has already built.

use std::collections::HashMap;

use sm_cst::{DeclKind, Language, NodeId, SourceTree};

/// The declaration kinds a duplicate of which is a bug rather than a style.
///
/// Locals, parameters and type parameters are deliberately absent: shadowing
/// and reuse across sibling scopes are normal, and this check has no scope
/// model to tell those apart from a real clash.
fn is_member(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Method
            | DeclKind::Constructor
            | DeclKind::Field
            | DeclKind::Property
            | DeclKind::Class
            | DeclKind::Interface
            | DeclKind::Enum
            | DeclKind::EnumMember
            | DeclKind::Record
            | DeclKind::Annotation
            | DeclKind::Function
            | DeclKind::TypeAlias
    )
}

/// The first duplicate the merge introduced, described for a human, or `None`.
pub fn introduced_duplicate(
    output: &SourceTree,
    inputs: [&SourceTree; 3],
    lang: &dyn Language,
) -> Option<String> {
    let out = keys(output, lang);
    if out.is_empty() {
        return None;
    }
    let before: [HashMap<String, usize>; 3] = [
        keys(inputs[0], lang),
        keys(inputs[1], lang),
        keys(inputs[2], lang),
    ];

    // Deterministic: the same output must always name the same duplicate, and
    // a `HashMap`'s iteration order is not that.
    let mut offenders: Vec<(&String, usize, usize)> = out
        .iter()
        .filter(|&(_, &n)| n > 1)
        .filter_map(|(key, &n)| {
            let most = before
                .iter()
                .map(|m| m.get(key).copied().unwrap_or(0))
                .max();
            let most = most.unwrap_or(0);
            (n > most).then_some((key, n, most))
        })
        .collect();
    offenders.sort_by(|a, b| a.0.cmp(b.0));

    offenders.first().map(|(key, n, most)| {
        format!(
            "a clean merge produced {n} declarations of {key}, and no input had more than {most}"
        )
    })
}

/// Every member declaration in the tree, keyed as the module docs describe.
fn keys(tree: &SourceTree, lang: &dyn Language) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    // `owner[id]` is the name chain in force *inside* node `id`. Filled in a
    // forward scan: a parent's arena index is always below its children's.
    let mut owner: Vec<Option<String>> = vec![None; tree.len()];

    for id in tree.ids() {
        let inherited = owner[id.index()].clone();
        let mut inside = inherited.clone();

        if let Some(kind) = lang.declaration_kind(tree, id)
            && is_member(kind)
            && let Some(name) = lang.declared_name(tree, id).map(|n| tree.node_bytes(n))
        {
            let name = String::from_utf8_lossy(name);
            let path = match &inherited {
                Some(prefix) => format!("{prefix}.{name}"),
                None => name.to_string(),
            };
            let key = format!("`{path}{}` ({})", params(tree, id), kind.tag());
            *out.entry(key).or_insert(0) += 1;
            // A type's members are qualified by the type's name; a method's
            // body is not somewhere this check descends usefully, but the
            // chain costs nothing and keeps nested classes apart.
            inside = Some(path);
        }

        for child in tree.children(id) {
            owner[child.index()] = inside.clone();
        }
    }
    out
}

/// The declaration's parameter list with **all** whitespace removed, or the
/// empty string when it has none.
///
/// All of it, not collapsed: the point is that reformatting a signature must
/// not hide a duplicate, and `(int a, String b)` versus `(int  a ,  String b)`
/// is reformatting. Removing whitespace entirely also fuses the type and the
/// parameter name (`inta`), which is why two methods differing only in a
/// parameter *name* still read as distinct here — an under-approximation the
/// module docs record.
///
/// Field-name based rather than kind-name based: `tree-sitter-java` calls it
/// `parameters` on a `method_declaration`, `tree-sitter-typescript` calls it
/// `parameters` too, and a grammar that does not have the field simply
/// contributes nothing — which turns the key into "name only" for that kind,
/// the conservative answer.
fn params(tree: &SourceTree, id: NodeId) -> String {
    let Some(list) = tree.child_by_field_name(id, "parameters") else {
        return String::new();
    };
    String::from_utf8_lossy(tree.node_bytes(list))
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::introduced_duplicate;
    use sm_cst::{Language, SourceTree};

    fn java() -> &'static dyn Language {
        sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java is registered")
    }

    fn tree(src: &str) -> SourceTree {
        sm_cst::parse(src.as_bytes(), java()).expect("parse")
    }

    fn check(base: &str, ours: &str, theirs: &str, output: &str) -> Option<String> {
        let (b, o, t, out) = (tree(base), tree(ours), tree(theirs), tree(output));
        introduced_duplicate(&out, [&b, &o, &t], java())
    }

    #[test]
    fn the_druid_case_is_caught() {
        // The real corpus case: theirs rewrote `restart`'s body, the merge
        // emitted both copies. It parses, and every token came from an input.
        let base = "class C { public void restart() { ds.restart(); } }";
        let ours = "class C { public void restart() { ds.restart(); } void x() {} }";
        let theirs = "class C { public void restart() { ds.resetState(); } }";
        let out = "class C { public void restart() { ds.restart(); } \
                   public void restart() { ds.resetState(); } void x() {} }";
        let err = check(base, ours, theirs, out).expect("the duplicate must be caught");
        assert!(err.contains("restart"), "{err}");
        assert!(err.contains("no input had more than 1"), "{err}");
    }

    #[test]
    fn an_honest_union_of_different_methods_passes() {
        let base = "class C { void a() {} }";
        let ours = "class C { void a() {} void b() {} }";
        let theirs = "class C { void a() {} void c() {} }";
        let out = "class C { void a() {} void b() {} void c() {} }";
        assert_eq!(check(base, ours, theirs, out), None);
    }

    #[test]
    fn a_real_overload_passes() {
        let base = "class C { void f(int a) {} }";
        let ours = "class C { void f(int a) {} void f(String s) {} }";
        let theirs = "class C { void f(int a) {} }";
        let out = "class C { void f(int a) {} void f(String s) {} }";
        assert_eq!(check(base, ours, theirs, out), None);
    }

    #[test]
    fn a_duplicate_that_was_already_in_an_input_is_not_ours_to_report() {
        // Generated code does this. The merge did not make it worse, so the
        // merge is not the thing to blame.
        let src = "class C { void f() {} void f() {} }";
        assert_eq!(check(src, src, src, src), None);
    }

    #[test]
    fn parameter_text_is_compared_modulo_whitespace() {
        let base = "class C { }";
        let ours = "class C { void f(int  a ,  String b) {} }";
        let theirs = "class C { void f(int a, String b) { return; } }";
        let out = "class C { void f(int  a ,  String b) {} void f(int a, String b) { return; } }";
        assert!(
            check(base, ours, theirs, out).is_some(),
            "reformatting a parameter list must not hide a duplicate"
        );
    }

    #[test]
    fn same_name_in_two_different_classes_is_not_a_duplicate() {
        let base = "class A { } class B { }";
        let ours = "class A { void f() {} } class B { }";
        let theirs = "class A { } class B { void f() {} }";
        let out = "class A { void f() {} } class B { void f() {} }";
        assert_eq!(
            check(base, ours, theirs, out),
            None,
            "the enclosing type's name is part of the key"
        );
    }

    #[test]
    fn duplicate_fields_and_types_are_caught_too() {
        let base = "class C { }";
        let ours = "class C { int x; }";
        let theirs = "class C { String x; }";
        let out = "class C { int x; String x; }";
        let err = check(base, ours, theirs, out).expect("a duplicate field is a duplicate");
        assert!(err.contains(".x"), "{err}");
    }

    #[test]
    fn locals_and_parameters_are_out_of_scope() {
        // Two `int i` in sibling blocks is ordinary Java, and this check has no
        // scope model to tell that from a clash.
        let src = "class C { void f() { { int i = 0; } { int i = 1; } } }";
        assert_eq!(check("class C { }", src, src, src), None);
    }

    #[test]
    fn the_offender_reported_is_stable_across_runs() {
        let base = "class C { }";
        let ours = "class C { void a() {} void b() {} }";
        let theirs = "class C { void a() { return; } void b() { return; } }";
        let out = "class C { void a() {} void a() { return; } void b() {} void b() { return; } }";
        let first = check(base, ours, theirs, out).expect("a duplicate");
        for _ in 0..20 {
            assert_eq!(check(base, ours, theirs, out), Some(first.clone()));
        }
    }
}
