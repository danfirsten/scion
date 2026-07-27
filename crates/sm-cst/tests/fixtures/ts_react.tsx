/** A React-ish component file, to exercise the TSX grammar. */

import "./styles.css";

import { useCallback, useMemo, useState } from "react";
import type { ReactNode } from "react";

export interface TodoItem {
  id: string;
  title: string;
  done: boolean;
}

interface TodoListProps {
  items: readonly TodoItem[];
  onToggle: (id: string) => void;
  children?: ReactNode;
}

const EMPTY_MESSAGE = "Nothing to do";

/** One row. Destructured props — an `object_pattern`, deliberately Ordered. */
function TodoRow({ item, onToggle }: { item: TodoItem; onToggle: (id: string) => void }) {
  // JSX children are Ordered: position is the reconciliation key.
  return (
    <li className={item.done ? "done" : "pending"}>
      <input
        type="checkbox"
        checked={item.done}
        onChange={() => onToggle(item.id)}
      />
      <span>{item.title}</span>
    </li>
  );
}

export function TodoList({ items, onToggle, children }: TodoListProps) {
  const [filter, setFilter] = useState<"all" | "open">("all");

  const visible = useMemo(
    () => (filter === "all" ? items : items.filter((item) => !item.done)),
    [filter, items],
  );

  const toggleFilter = useCallback(() => {
    setFilter((current) => (current === "all" ? "open" : "all"));
  }, []);

  if (visible.length === 0) {
    return <p className="empty">{EMPTY_MESSAGE}</p>;
  }

  return (
    <section>
      <h2>Things to do</h2>
      {/* An attribute list is Ordered too: a later duplicate wins and a
          spread interleaves. */}
      <button onClick={toggleFilter} disabled={items.length === 0}>
        {filter === "all" ? "Show open" : "Show all"}
      </button>
      <ul>
        {visible.map((item) => (
          <TodoRow key={item.id} item={item} onToggle={onToggle} />
        ))}
      </ul>
      {children}
    </section>
  );
}

export default TodoList;
