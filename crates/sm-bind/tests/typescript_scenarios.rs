//! The same check, through `TypeScriptLanguage`, to show the analysis is a
//! property of the `Language` trait rather than of Java.
//!
//! Nothing in `sm-bind` mentions either language. What changes between this file
//! and `java_scenarios.rs` is the `Language` implementation: TypeScript reports
//! `MemberVisibility::ReceiverOnly` (a bare `x` in a method is *not* a field),
//! has function-hoisted `var`, and routes `property_identifier` to
//! `IdentifierRole::MemberRef`.

mod support;

use support::{run, typescript};

/// The headline case in TypeScript: a renamed function and a new call to the
/// old name.
#[test]
fn rename_on_one_side_new_call_on_the_other() {
    let base = "\
import { Db } from \"./db\";

export function getUser(id: string) {
  return Db.find(id);
}

export function cached(id: string) {
  return getUser(id);
}
";
    let ours = "\
import { Db } from \"./db\";

export function fetchUser(id: string) {
  return Db.find(id);
}

export function cached(id: string) {
  return fetchUser(id);
}
";
    let theirs = "\
import { Db } from \"./db\";

export function getUser(id: string) {
  return Db.find(id);
}

export function cached(id: string) {
  return getUser(id);
}

export function first() {
  return getUser(\"1\");
}
";
    if let Some(clean) = support::git_line_merge_is_clean(base, ours, theirs) {
        assert!(clean, "the premise requires git's line merge to be clean");
    }
    let s = run(typescript(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    insta::assert_snapshot!(s.render());
}

/// A renamed `interface` used in a type position the other branch added.
#[test]
fn renamed_interface_with_a_new_type_reference() {
    let base = "\
interface Options {
  limit: number;
}

export function make(): Options {
  return { limit: 1 };
}
";
    let ours = "\
interface Settings {
  limit: number;
}

export function make(): Settings {
  return { limit: 1 };
}
";
    let theirs = "\
interface Options {
  limit: number;
}

export function make(): Options {
  return { limit: 1 };
}

export function makeTwo(): Options {
  return { limit: 2 };
}
";
    let s = run(typescript(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    insta::assert_snapshot!(s.render());
}

/// A `const` introduced by one branch captures a module-level name the other
/// branch's new statement was using.
#[test]
fn a_new_const_captures_a_module_level_name() {
    let base = "\
const limit = 10;

export function run(xs: number[]) {
  const seen = 0;
  return seen;
}
";
    let ours = "\
const limit = 10;

export function run(xs: number[]) {
  const seen = 0;
  return seen + limit;
}
";
    let theirs = "\
const limit = 10;

export function run(xs: number[]) {
  const limit = xs.length;
  const seen = 0;
  return seen;
}
";
    let s = run(typescript(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    insta::assert_snapshot!(s.render());
}

// ---------------------------------------------------------------- negatives

/// **The finding that motivated `IdentifierRole`.** Every one of these names is
/// a `property_identifier` resolved against a receiver's type. If they were
/// treated as lexical references, this file alone would report six broken
/// references.
#[test]
fn negative_property_access_is_never_reported() {
    let base = "\
export function render(user: User) {
  console.log(user.getName());
}
";
    let ours = "\
export function render(user: User) {
  console.log(user.getName());
  console.log(user.getEmail());
}
";
    let theirs = "\
export function render(user: User) {
  console.log(user.getName());
}

export function render2(user: User) {
  console.warn(user.nickname, user.profile.avatar);
}
";
    let s = run(typescript(), base, ours, theirs);
    s.assert_silent();
    // And prove the check really did look: it saw references, they just all
    // came out unresolved on both sides.
    assert!(s.report.stats.references > 0);
}

/// A class member is only reachable through `this`, so a method parameter
/// sharing a field's name is not a capture, and a renamed field is not a broken
/// reference in a language where nothing bare refers to it.
#[test]
fn negative_class_members_are_receiver_only() {
    let base = "\
export class Repo {
  private count = 0;

  bump(count: number) {
    this.count = count;
  }
}
";
    let ours = "\
export class Repo {
  private total = 0;

  bump(count: number) {
    this.total = count;
  }
}
";
    let theirs = "\
export class Repo {
  private count = 0;

  bump(count: number) {
    this.count = count;
  }

  reset(count: number) {
    this.count = count;
  }
}
";
    run(typescript(), base, ours, theirs).assert_silent();
}

/// Imported names resolve to the import binding in every revision; two branches
/// each importing one more name must stay silent.
#[test]
fn negative_disjoint_import_additions() {
    let base = "\
import { a } from \"./m\";

export const x = a();

export const tail = 0;
";
    // Disjoint additions at different anchors, so the *syntactic* merge is
    // clean and only the name analysis is under test.
    let ours = "\
import { a, b } from \"./m\";

export const x = a();
export const y = b();

export const tail = 0;
";
    let theirs = "\
import { a, c } from \"./m\";

export const x = a();

export const tail = 0;
export const z = c();
";
    let s = run(typescript(), base, ours, theirs);
    assert!(s.tree_merge_is_clean(), "{}", s.merged_text);
    s.assert_silent();
}
