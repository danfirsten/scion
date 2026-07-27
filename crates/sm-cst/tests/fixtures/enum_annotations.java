package com.example.inventory;

import java.lang.annotation.Documented;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;

@Documented
@Retention(RetentionPolicy.RUNTIME)
@interface Stable {
    String since() default "1.0";

    int weight() default 0;
}

/** Shipment priority. Ordinals are persisted, so order is load-bearing. */
@Stable(since = "1.2")
public enum Priority {
    @Deprecated
    LOWEST(0),

    /** The default. */
    NORMAL(50),

    @Stable(since = "1.4", weight = 2)
    URGENT(100);

    private final int rank;

    Priority(int rank) {
        this.rank = rank;
    }

    public int rank() {
        return rank;
    }

    @Override
    public String toString() {
        return name() + "(" + rank + ")";
    }
}
