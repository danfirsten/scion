// g01 copyright header, separated from `package` by a blank line -> floating
// g02 second line of that header run -> floating

package com.example.gallery;

// g03 glued to the import below -> leading(import java.util.List)
import java.util.List;

import java.util.Map; // g04 same line as the import -> trailing(import java.util.Map)

// g05 first line of a run
// g06 second line of the same run
/**
 * g07 Javadoc closing the run; all three chain to the class -> leading(class)
 */
public class Gallery { // g08 the `{` is anonymous, so nothing precedes it that
                       //     could own it; it chains forward -> leading(FLOOR)
    /** g09 Javadoc for a field, closing that chain -> leading(FLOOR) */
    static final int FLOOR = 0;

    // g10 separated from what follows by a blank line -> floating

    /* g11 block comment on its own line -> leading(ceiling) */
    private int ceiling = 10; // g12 trailing(ceiling)

    /* g13 a block comment that
       spans several lines and ends on the line before the field
       -> leading(depth), because the gap is measured from its END */
    private int depth = 1;

    private int width = 2; /* g14 a block comment that starts on the same line
                              as the field ends and runs on
                              -> trailing(width), because the trailing check
                              uses its START line */

    /** g15 Javadoc above an annotated method -> leading(annotated) */
    @Deprecated
    @SuppressWarnings("unused")
    // g16 wedged between the annotations and the return type: a child of
    // `modifiers`, where the only candidate owners are the annotations
    // themselves -> floating
    public void annotated() {
        // g17 first statement's leading comment -> leading(int local = 1)
        int local = 1; // g18 trailing(local)

        // g19 after a blank line but glued to what follows -> leading(return)
        if (local > FLOOR) {
            return;
        }

        // g20 last child of the method body, nothing after it but `}`
        // -> floating, and it does NOT reach the next class member
    }

    int sum(int a, int b) {
        return add(
                a, // g21 after a `,`, so the trailing rule cannot see `a`
                   // -> leading(b), and g22 here chains to the same owner
                b);
    }

    private int add(int a, int b) {
        return a + b;
    }

    // g23 comments inside a nested block stay inside it
    void nested() {
        {
            int inner = 0; // g24 trailing(inner), owner is inside the inner block
            // g25 last in the inner block -> floating
        }
        int outer = 1;
    }

    // g26 Ünïcödé: multi-byte characters before a comment must not shift lines
    String naïve = "café ☕"; // g27 trailing(naïve)

    // g28 last member of the class body -> floating
}
// g29 after the class, on its own line -> floating
