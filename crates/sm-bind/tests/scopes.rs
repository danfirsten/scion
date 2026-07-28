//! Unit tests for scope construction and resolution.
//!
//! The scenario suites prove the *check* works end to end. These prove the
//! layer underneath it implements the rules it claims to: shadowing, Java's
//! order-independent member visibility, TypeScript's block scoping and
//! hoisting, and the boundary between them.

mod support;

use support::{binding_of, bindings_dump, decls_dump, java, typescript};

// ------------------------------------------------------------------- Java

/// Java resolves members by name, not by position: a method may use a field
/// declared below it, and call a method declared below it.
#[test]
fn java_members_are_visible_before_their_declaration() {
    let src = "\
class C {
    int use() {
        return later + helper();
    }

    private int later = 1;

    private int helper() {
        return 2;
    }
}
";
    assert_eq!(
        binding_of(java(), src, "later", 0),
        "field `later` in class_declaration @ line 6"
    );
    assert_eq!(
        binding_of(java(), src, "helper", 0),
        "method `helper` in class_declaration @ line 8"
    );
}

/// A local is visible from its own declaration onwards, not before it. Before
/// it, the enclosing field of the same name is what a name means.
#[test]
fn java_a_local_shadows_a_field_only_from_its_declaration_point() {
    let src = "\
class C {
    private int value = 1;

    int f() {
        int before = value;
        int value = 2;
        int after = value;
        return before + after;
    }
}
";
    assert_eq!(
        binding_of(java(), src, "value", 0),
        "field `value` in class_declaration @ line 2"
    );
    assert_eq!(
        binding_of(java(), src, "value", 1),
        "local variable `value` in block @ line 6"
    );
}

/// Innermost wins, through several levels.
#[test]
fn java_shadowing_is_innermost_first() {
    let src = "\
class C {
    private int x = 0;

    void f(int x) {
        int a = x;
        {
            int x = 2;
            int b = x;
        }
    }
}
";
    assert_eq!(
        binding_of(java(), src, "x", 0),
        "parameter `x` in method_declaration @ line 4"
    );
    assert_eq!(
        binding_of(java(), src, "x", 1),
        "local variable `x` in block @ line 7"
    );
}

/// A nested class sees the enclosing class's members.
#[test]
fn java_a_nested_class_sees_the_outer_classs_members() {
    let src = "\
class Outer {
    private int shared = 1;

    class Inner {
        int f() {
            return shared;
        }
    }
}
";
    assert_eq!(
        binding_of(java(), src, "shared", 0),
        "field `shared` in class_declaration @ line 2"
    );
}

/// An import binds its *simple* name at file scope. A wildcard import binds
/// nothing, which is exactly why unresolved names are never reported.
#[test]
fn java_imports_bind_their_simple_name_and_wildcards_bind_nothing() {
    let src = "\
import java.util.List;
import java.util.*;

class C {
    List a;
    Set b;
}
";
    assert_eq!(
        binding_of(java(), src, "List", 0),
        "import `List` in program @ line 1"
    );
    assert_eq!(binding_of(java(), src, "Set", 0), "unresolved");
}

/// A type parameter is in scope for the declaration that introduced it, and not
/// outside it.
#[test]
fn java_type_parameters_scope_to_their_declaration() {
    let src = "\
class C {
    <T> T pick(T a) {
        T local = a;
        return local;
    }

    T outside;
}
";
    assert_eq!(
        binding_of(java(), src, "T", 0),
        "type parameter `T` in method_declaration @ line 2"
    );
    // The last `T` is the field's type, outside the method.
    let n = src.matches('T').count();
    assert!(n > 3);
    assert_eq!(binding_of(java(), src, "T", 3), "unresolved");
}

/// A `catch` parameter lives in the catch clause, not in the enclosing block.
#[test]
fn java_a_catch_parameter_scopes_to_its_clause() {
    let src = "\
class C {
    void f() {
        try {
            g();
        } catch (Exception e) {
            h(e);
        }
        int after = 1;
    }

    void g() {}
    void h(Object o) {}
}
";
    assert_eq!(
        binding_of(java(), src, "e", 0),
        "parameter `e` in catch_clause @ line 5"
    );
}

/// Java keeps methods in their own namespace: `count` and `count()` in the same
/// class are different declarations, and each reference finds the right one.
#[test]
fn java_a_field_and_a_method_may_share_a_name() {
    let src = "\
class C {
    private int count = 1;

    int count() {
        return 2;
    }

    int f() {
        return count + count();
    }
}
";
    assert_eq!(
        binding_of(java(), src, "count", 0),
        "field `count` in class_declaration @ line 2"
    );
    assert_eq!(
        binding_of(java(), src, "count", 1),
        "method `count` in class_declaration @ line 4"
    );
}

/// The broad view. A regression anywhere in Java's role table or scope rules
/// shows up here even if nobody wrote the assertion for it.
#[test]
fn java_binding_gallery() {
    let src = "\
package com.example;

import java.util.List;
import java.util.*;

class Repo {
    private final Store store;
    private static final int LIMIT = 10;

    Repo(Store store) {
        this.store = store;
    }

    User getUser(String id) {
        User u = store.find(id);
        if (u == null) {
            u = fallback(id);
            System.out.println(id);
        }
        return u;
    }

    private User fallback(String id) {
        int tries = LIMIT;
        for (int i = 0; i < tries; i++) {
            log(i);
        }
        outer:
        while (true) {
            break outer;
        }
        return null;
    }

    void log(int i) {}

    enum Color { RED, GREEN }

    class Inner {
        void g() {
            log(LIMIT);
        }
    }
}
";
    insta::assert_snapshot!(bindings_dump(java(), src));
}

/// The declaration side of the same gallery.
#[test]
fn java_declaration_gallery() {
    let src = "\
package com.example;

import java.util.List;
import static java.util.Objects.requireNonNull;
import java.util.*;

class Repo<T extends Number> {
    private int field;

    Repo(int p) {
        int local = p;
    }

    <R> R method(R r, int... rest) {
        try (AutoCloseable c = open()) {
        } catch (Exception e) {
        }
        return r;
    }

    enum Color { RED, GREEN }

    record Pt(int x, int y) {}

    interface I { void h(); }

    @interface A { String value(); }
}
";
    insta::assert_snapshot!(decls_dump(java(), src));
}

// ------------------------------------------------------------- TypeScript

/// `let` and `const` are block-scoped and take effect from their declaration.
#[test]
fn typescript_let_and_const_are_block_scoped() {
    let src = "\
const value = 1;

export function f() {
  const before = value;
  {
    const value = 2;
    const inner = value;
  }
  const after = value;
}
";
    assert_eq!(
        binding_of(typescript(), src, "value", 0),
        "local variable `value` in program @ line 1"
    );
    assert_eq!(
        binding_of(typescript(), src, "value", 1),
        "local variable `value` in statement_block @ line 6"
    );
    assert_eq!(
        binding_of(typescript(), src, "value", 2),
        "local variable `value` in program @ line 1"
    );
}

/// `var` ignores blocks and hoists to the nearest function scope.
#[test]
fn typescript_var_is_function_scoped() {
    let src = "\
export function f() {
  {
    var hoisted = 1;
  }
  return hoisted;
}
";
    assert_eq!(
        binding_of(typescript(), src, "hoisted", 0),
        "var `hoisted` in function_declaration @ line 3"
    );
}

/// A function declaration is visible above itself.
#[test]
fn typescript_function_declarations_are_hoisted() {
    let src = "\
export function caller() {
  return callee();
}

function callee() {
  return 1;
}
";
    assert_eq!(
        binding_of(typescript(), src, "callee", 0),
        "function `callee` in program @ line 5"
    );
}

/// **The TypeScript half of the `is_identifier` finding.** A class field is
/// reachable only through a receiver, so a bare name inside a method does *not*
/// see it — otherwise a parameter that shares a field's name would look like a
/// capture on every merge.
#[test]
fn typescript_class_members_are_not_in_the_lexical_scope() {
    let src = "\
const count = 99;

export class C {
  private count = 0;

  f() {
    return count;
  }
}
";
    assert_eq!(
        binding_of(typescript(), src, "count", 0),
        "local variable `count` in program @ line 1"
    );
}

/// Imports bind at module scope, aliases bind the alias.
#[test]
fn typescript_imports_bind_at_module_scope() {
    let src = "\
import def, * as ns from \"./m\";
import { a, b as renamed } from \"./n\";

export const x = [def, ns, a, renamed];
";
    insta::assert_snapshot!(decls_dump(typescript(), src));
}

/// Destructuring patterns declare each of their leaves.
#[test]
fn typescript_destructuring_declares_its_leaves() {
    let src = "\
export function f(o: any, xs: number[]) {
  const { a, b: renamed } = o;
  const [p, ...rest] = xs;
  return [a, renamed, p, rest];
}
";
    assert_eq!(
        binding_of(typescript(), src, "a", 0),
        "local variable `a` in statement_block @ line 2"
    );
    assert_eq!(
        binding_of(typescript(), src, "renamed", 0),
        "local variable `renamed` in statement_block @ line 2"
    );
    assert_eq!(
        binding_of(typescript(), src, "p", 0),
        "local variable `p` in statement_block @ line 3"
    );
    assert_eq!(
        binding_of(typescript(), src, "rest", 0),
        "local variable `rest` in statement_block @ line 3"
    );
}

/// `for (const x of xs)` binds `x`; a `catch` parameter binds in its clause.
#[test]
fn typescript_for_of_and_catch_bind_their_names() {
    let src = "\
export function f(xs: number[]) {
  for (const item of xs) {
    use(item);
  }
  try {
    risky();
  } catch (err) {
    use(err);
  }
}
";
    assert_eq!(
        binding_of(typescript(), src, "item", 0),
        "local variable `item` in for_in_statement @ line 2"
    );
    assert_eq!(
        binding_of(typescript(), src, "err", 0),
        "parameter `err` in catch_clause @ line 7"
    );
}

/// The broad view, TypeScript side.
#[test]
fn typescript_binding_gallery() {
    let src = "\
import { Db } from \"./db\";

const LIMIT = 10;
type Id = string;

interface Row {
  id: Id;
}

export function load(id: Id): Row {
  const row = Db.find(id);
  console.log(row.id, LIMIT);
  return row;
}

export class Repo {
  private cache = new Map<string, Row>();

  get(id: Id): Row {
    return this.cache.get(id) ?? load(id);
  }
}
";
    insta::assert_snapshot!(bindings_dump(typescript(), src));
}
