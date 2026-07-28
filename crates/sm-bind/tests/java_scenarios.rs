//! The constructed semantic-conflict suite for Java (SPEC.md §7, M6 exit).
//!
//! Every test runs a real three-way merge and pins the resulting
//! `SemanticConflict` list with `insta`. The negatives matter at least as much
//! as the positives: this analysis is only worth shipping if it is quiet.

mod support;

use support::{java, run};

// ---------------------------------------------------------------- positives

/// **The headline case.** SPEC.md §1: "Branch A renames `getUser`; branch B
/// adds a call to `getUser`. The edits touch different lines, git merges clean,
/// the build breaks."
#[test]
fn rename_on_one_side_new_call_on_the_other() {
    let base = "\
class Repo {
    private final Store store;

    User getUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return getUser(id);
    }
}
";
    // We rename the method and every call site we can see.
    let ours = "\
class Repo {
    private final Store store;

    User fetchUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return fetchUser(id);
    }
}
";
    // They add a new method that calls the old name. Different lines entirely.
    let theirs = "\
class Repo {
    private final Store store;

    User getUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return getUser(id);
    }

    User first() {
        return getUser(\"1\");
    }
}
";

    // The premise of the whole milestone: both merges are clean and wrong.
    if let Some(clean) = support::git_line_merge_is_clean(base, ours, theirs) {
        assert!(clean, "the premise requires git's line merge to be clean");
    }
    let s = run(java(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "tree merge should also be clean");
    assert!(
        s.merged_text.contains("fetchUser") && s.merged_text.contains("getUser"),
        "the merged file should contain both names:\n{}",
        s.merged_text
    );

    insta::assert_snapshot!(s.render());
}

/// The mirror. SPEC.md §5 requires the *decision* to be symmetric; the labels
/// legitimately swap.
#[test]
fn rename_on_one_side_new_call_on_the_other_mirrored() {
    let base = "\
class Repo {
    private final Store store;

    User getUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return getUser(id);
    }
}
";
    let renamed = "\
class Repo {
    private final Store store;

    User fetchUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return fetchUser(id);
    }
}
";
    let new_call = "\
class Repo {
    private final Store store;

    User getUser(String id) {
        return store.find(id);
    }

    User cached(String id) {
        return getUser(id);
    }

    User first() {
        return getUser(\"1\");
    }
}
";
    let s = run(java(), base, new_call, renamed);
    assert!(s.tree_merge_is_clean());
    insta::assert_snapshot!(s.render());
}

/// One branch deletes a private helper; the other adds a call to it.
#[test]
fn deleted_helper_with_a_new_call_to_it() {
    let base = "\
class Repo {
    private int helper(int x) {
        return x + 1;
    }

    int a() {
        return helper(1);
    }
}
";
    let ours = "\
class Repo {
    int a() {
        return 2;
    }
}
";
    let theirs = "\
class Repo {
    private int helper(int x) {
        return x + 1;
    }

    int a() {
        return helper(1);
    }

    int b() {
        return helper(7);
    }
}
";
    let s = run(java(), base, ours, theirs);
    insta::assert_snapshot!(s.render());
}

/// One branch renames a field; the other adds a method that uses the old name.
#[test]
fn renamed_field_with_a_new_use_of_the_old_name() {
    let base = "\
class Counter {
    private int count = 0;

    void bump() {
        count = count + 1;
    }
}
";
    let ours = "\
class Counter {
    private int total = 0;

    void bump() {
        total = total + 1;
    }
}
";
    let theirs = "\
class Counter {
    private int count = 0;

    void bump() {
        count = count + 1;
    }

    int report() {
        return count;
    }
}
";
    let s = run(java(), base, ours, theirs);
    insta::assert_snapshot!(s.render());
}

/// One branch renames a class; the other adds a declaration of the old type.
#[test]
fn renamed_class_with_a_new_reference_to_the_old_type() {
    let base = "\
class Holder {
    Foo make() {
        return null;
    }
}

class Foo {
    int x;
}
";
    let ours = "\
class Holder {
    Bar make() {
        return null;
    }
}

class Bar {
    int x;
}
";
    let theirs = "\
class Holder {
    Foo make() {
        return null;
    }

    int size() {
        Foo f = make();
        return 1;
    }
}

class Foo {
    int x;
}
";
    let s = run(java(), base, ours, theirs);
    insta::assert_snapshot!(s.render());
}

/// **The classic silent capture.** We add a statement that uses the field
/// `log`; they add a local `log` above it. Neither diff shows a problem, both
/// merges are clean, and the merged method logs to the wrong logger.
#[test]
fn a_new_local_captures_a_reference_that_meant_the_field() {
    let base = "\
class Service {
    private Logger log;
    private int calls;

    void handle(String msg) {
        calls = calls + 1;
    }
}
";
    // We append a statement whose `log` is the field.
    let ours = "\
class Service {
    private Logger log;
    private int calls;

    void handle(String msg) {
        calls = calls + 1;
        log.info(msg);
    }
}
";
    // They prepend a local that shadows it.
    let theirs = "\
class Service {
    private Logger log;
    private int calls;

    void handle(String msg) {
        Logger log = Logger.forRequest(msg);
        calls = calls + 1;
    }
}
";
    if let Some(clean) = support::git_line_merge_is_clean(base, ours, theirs) {
        assert!(clean, "the premise requires git's line merge to be clean");
    }
    let s = run(java(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    insta::assert_snapshot!(s.render());
}

/// A rename inside a method captures a reference the other branch added.
///
/// They add `return result;` meaning the *field*; we rename a local `tmp` to
/// `result` earlier in the same method. Merged, their `result` is our local.
#[test]
fn a_renamed_local_captures_a_new_reference_to_the_field() {
    let base = "\
class Calc {
    private int result;

    int compute(int n) {
        int tmp = n * 2;
        record(tmp);
        return 0;
    }

    void record(int v) {
    }
}
";
    let ours = "\
class Calc {
    private int result;

    int compute(int n) {
        int result = n * 2;
        record(result);
        return 0;
    }

    void record(int v) {
    }
}
";
    let theirs = "\
class Calc {
    private int result;

    int compute(int n) {
        int tmp = n * 2;
        record(tmp);
        return result;
    }

    void record(int v) {
    }
}
";
    let s = run(java(), base, ours, theirs);
    insta::assert_snapshot!(s.render());
}

// ---------------------------------------------------------------- negatives

/// The same rename, but their new code calls the **new** name. Nothing is
/// broken and nothing may be reported.
#[test]
fn negative_rename_where_the_new_call_uses_the_new_name() {
    let base = "\
class Repo {
    User getUser(String id) {
        return null;
    }
}
";
    let ours = "\
class Repo {
    User fetchUser(String id) {
        return null;
    }
}
";
    let theirs = "\
class Repo {
    User getUser(String id) {
        return null;
    }

    User first() {
        return fetchUser(\"1\");
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// Library and JDK names resolve to nothing in *every* revision, so the
/// differential rule keeps them silent. This is the single most important
/// negative in the suite.
#[test]
fn negative_jdk_and_library_names() {
    let base = "\
import java.util.Objects;

class Util {
    void a(String s) {
        System.out.println(s);
    }
}
";
    let ours = "\
import java.util.Objects;

class Util {
    void a(String s) {
        System.out.println(s);
        System.err.println(s);
    }
}
";
    let theirs = "\
import java.util.Objects;

class Util {
    void a(String s) {
        System.out.println(s);
    }

    void b(String s) {
        Objects.requireNonNull(s);
        System.out.println(s.trim());
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// A member selected through a receiver is never resolved, so a method that
/// only exists on some other file's type is never reported.
#[test]
fn negative_member_access_through_a_receiver() {
    let base = "\
class View {
    void render(User user) {
        show(user.getName());
    }

    void show(String s) {
    }
}
";
    let ours = "\
class View {
    void render(User user) {
        show(user.getName());
        show(user.getEmail());
    }

    void show(String s) {
    }
}
";
    let theirs = "\
class View {
    void render(User user) {
        show(user.getName());
    }

    void show(String s) {
    }

    void render2(User user) {
        show(user.getNickname());
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// A wildcard import declares nothing this analysis can see, so names it
/// supplies stay unresolved on both sides and stay silent.
#[test]
fn negative_wildcard_import_usage() {
    let base = "\
import java.util.*;

class Bag {
    List<String> items = new ArrayList<>();
}
";
    let ours = "\
import java.util.*;

class Bag {
    List<String> items = new ArrayList<>();
    Set<String> seen = new HashSet<>();
}
";
    let theirs = "\
import java.util.*;

class Bag {
    List<String> items = new ArrayList<>();

    Map<String, String> index() {
        return new HashMap<>();
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// Two branches adding independent methods is the case the *syntactic* merge
/// exists to resolve. It must not acquire a semantic complaint.
#[test]
fn negative_both_sides_add_independent_methods() {
    let base = "\
class Repo {
    private int n;

    int a() {
        return n;
    }
}
";
    let ours = "\
class Repo {
    private int n;

    int a() {
        return n;
    }

    int b() {
        return a() + n;
    }
}
";
    let theirs = "\
class Repo {
    private int n;

    int a() {
        return n;
    }

    int c() {
        return a() * n;
    }
}
";
    let s = run(java(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    s.assert_silent();
}

/// The rename happened in the **ancestor**, so both branches already saw it.
/// There is nothing differential to report.
#[test]
fn negative_rename_already_in_base() {
    let base = "\
class Repo {
    User fetchUser(String id) {
        return null;
    }

    User a() {
        return fetchUser(\"a\");
    }
}
";
    let ours = "\
class Repo {
    User fetchUser(String id) {
        return null;
    }

    User a() {
        return fetchUser(\"a\");
    }

    User b() {
        return fetchUser(\"b\");
    }
}
";
    let theirs = "\
class Repo {
    User fetchUser(String id) {
        return null;
    }

    User a() {
        return fetchUser(\"a\");
    }

    User c() {
        return fetchUser(\"c\");
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// A parameter that shadows a field is ordinary Java and is present on both
/// sides; adding an unrelated method must not turn it into a capture.
#[test]
fn negative_pre_existing_shadowing_is_not_a_capture() {
    let base = "\
class Point {
    private int x;

    void setX(int x) {
        this.x = x;
    }
}
";
    let ours = "\
class Point {
    private int x;

    void setX(int x) {
        this.x = x;
    }

    int getX() {
        return x;
    }
}
";
    let theirs = "\
class Point {
    private int x;

    void setX(int x) {
        this.x = x;
    }

    void reset() {
        x = 0;
    }
}
";
    run(java(), base, ours, theirs).assert_silent();
}

/// A genuinely conflicting textual merge. The check must not crash, must not
/// report anything inside the conflict region, and must still examine the rest
/// of the file.
#[test]
fn a_textual_conflict_is_skipped_gracefully() {
    let base = "\
class Repo {
    int value() {
        return 1;
    }
}
";
    let ours = "\
class Repo {
    int value() {
        return 2;
    }
}
";
    let theirs = "\
class Repo {
    int value() {
        return 3;
    }
}
";
    let s = run(java(), base, ours, theirs);
    assert!(
        !s.tree_merge_is_clean(),
        "this scenario is supposed to conflict:\n{}",
        s.merged_text
    );
    assert!(
        s.merged_text.contains("<<<<<<<"),
        "expected markers:\n{}",
        s.merged_text
    );
    insta::assert_snapshot!(s.render());
}

/// A conflict elsewhere in the file must not stop the check from catching a
/// real semantic break in the part that merged cleanly.
#[test]
fn a_semantic_break_is_still_caught_next_to_a_textual_conflict() {
    let base = "\
class Repo {
    String mode() {
        return \"original\";
    }

    int helper() {
        return 5;
    }

    int a() {
        return helper();
    }
}
";
    let ours = "\
class Repo {
    String mode() {
        return \"ours-mode\";
    }

    int renamedHelper() {
        return 5;
    }

    int a() {
        return renamedHelper();
    }
}
";
    let theirs = "\
class Repo {
    String mode() {
        return \"theirs-mode\";
    }

    int helper() {
        return 5;
    }

    int a() {
        return helper();
    }

    int b() {
        return helper() + 1;
    }
}
";
    let s = run(java(), base, ours, theirs);
    assert!(!s.tree_merge_is_clean(), "{}", s.merged_text);
    insta::assert_snapshot!(s.render());
}
