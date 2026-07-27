// Copyright 2026 the semantic-merge authors.
// SPDX-License-Identifier: MIT OR Apache-2.0

// A side-effect-only import: no import clause, and the whole point of it is
// that it runs before everything below. This is why `program` is Ordered.
import "./polyfill";

import { readFile, writeFile } from "node:fs/promises";
import type { Stats } from "node:fs";
import * as path from "node:path";
import defaultExport, { alpha, beta as renamedBeta } from "./helpers";

export const DEFAULT_TIMEOUT_MS = 5_000; // milliseconds

/**
 * A store of user records, keyed by id.
 *
 * The JSDoc block above is a `comment` node, not a distinct `javadoc` kind —
 * tree-sitter-typescript has one comment kind for `//` and `/* *\/` alike.
 */
export class UserStore<T extends { id: string }> implements Readable<T> {
  /** Records by id. Initialised before `count`, and that order matters. */
  private readonly records = new Map<string, T>();

  // mutable, set by clear()
  private count: number = 0;

  static {
    // A static initialiser block. Runs in textual order with field
    // initialisers, which is the caveat class_body's Unordered carries.
    globalThis.__userStoreLoaded = true;
  }

  constructor(seed: readonly T[] = []) {
    for (const record of seed) {
      this.records.set(record.id, record);
      this.count += 1;
    }
  }

  get size(): number {
    return this.count;
  }

  public find(id: string): T | undefined {
    return this.records.get(id);
  }

  async load(file: string, encoding: BufferEncoding = "utf8"): Promise<void> {
    const raw = await readFile(path.resolve(file), encoding);
    const parsed = JSON.parse(raw) as T[];
    parsed.forEach((record) => this.records.set(record.id, record));
    this.count = this.records.size;
  }

  clear(): void {
    this.records.clear();
    this.count = 0;
  }
}

/** Anything that can be read out one record at a time. */
export interface Readable<T> {
  // Interface members are order-insensitive: they have no runtime existence.
  readonly size: number;
  find(id: string): T | undefined;
  clear(): void;
}

/** A type literal — the same child inventory as an interface body. */
export type Snapshot = {
  takenAt: number;
  records: readonly string[];
};

/**
 * Numeric enum members auto-increment from the previous one, so inserting a
 * member renumbers everything after it. That is why `enum_body` is Ordered.
 */
export enum Level {
  Debug,
  Info,
  Warn = 10,
  Error,
}

export const identity = <T,>(value: T): T => value;

export const describe = (level: Level, detail?: string): string =>
  detail === undefined ? Level[level] : `${Level[level]}: ${detail}`;

export function partition<T>(
  items: readonly T[],
  predicate: (item: T, index: number) => boolean,
): [T[], T[]] {
  const yes: T[] = [];
  const no: T[] = [];
  items.forEach((item, index) => {
    if (predicate(item, index)) {
      yes.push(item);
    } else {
      no.push(item);
    }
  });
  return [yes, no];
}

export namespace Diagnostics {
  export const enabled = true;

  export function report(message: string): void {
    if (!enabled) {
      return;
    }
    try {
      console.warn(message);
    } catch (error) {
      // Swallowed on purpose: reporting must never throw.
      void error;
    }
  }
}

// Object literals are Ordered: a later key overrides an earlier one, and a
// spread interleaves with explicit keys.
const defaults = {
  timeout: DEFAULT_TIMEOUT_MS,
  retries: 3,
  ...overrides,
  retries: 5,
};

label: for (const key in defaults) {
  if (key === "retries") {
    break label;
  }
}

export { defaults, partition as split };
export default UserStore;
