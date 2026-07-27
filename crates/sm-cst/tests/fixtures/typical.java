// Copyright 2026 the semantic-merge authors.
// SPDX-License-Identifier: MIT OR Apache-2.0

package com.example.inventory;

import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * A warehouse of {@link Item}s.
 *
 * <p>Deliberately mundane: this fixture exists to exercise imports, Javadoc,
 * line comments and block comments in one file.
 */
public class Warehouse {

    /** How many items we will hold before refusing more. */
    private static final int CAPACITY = 128;

    /* The backing store. Not thread safe. */
    private final List<Item> items = new ArrayList<>();

    private String name; // mutable, set by rename()

    public Warehouse(String name) {
        this.name = Objects.requireNonNull(name, "name");
    }

    /**
     * Adds an item.
     *
     * @param item the item to add, never null
     * @return true if the item was added
     */
    public boolean add(Item item) {
        if (items.size() >= CAPACITY) {
            // Full. Callers are expected to check this.
            return false;
        }
        items.add(item);
        return true;
    }

    public void rename(String newName) {
        this.name = newName;
    }

    // A trailing comment for the class, sort of.
    public int size() {
        return items.size();
    }
}
