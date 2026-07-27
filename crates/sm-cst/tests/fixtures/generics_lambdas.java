package com.example.inventory;

import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.BiFunction;
import java.util.function.Function;
import java.util.stream.Collectors;

public final class Pipelines<K extends Comparable<? super K>, V> {

    /** A nested static class with its own type parameters. */
    public static final class Pair<A, B> {
        private final A first;
        private final B second;

        private Pair(A first, B second) {
            this.first = first;
            this.second = second;
        }

        public static <A, B> Pair<A, B> of(A a, B b) {
            return new Pair<>(a, b);
        }

        public <C> Pair<A, C> mapSecond(Function<? super B, ? extends C> f) {
            return Pair.of(first, f.apply(second));
        }
    }

    /** A nested interface, to exercise interface_body. */
    public interface Reducer<T> {
        T combine(T left, T right);

        default T combineAll(List<T> values, T identity) {
            T acc = identity;
            for (T value : values) {
                acc = combine(acc, value);
            }
            return acc;
        }

        /** The same thing with a C-style loop, to exercise for_statement. */
        default T combineFirst(List<T> values, T identity, int count) {
            T acc = identity;
            for (int i = 0; i < count && i < values.size(); i++) {
                acc = combine(acc, values.get(i));
            }
            return acc;
        }
    }

    private final Map<K, List<V>> byKey;

    public Pipelines(Map<K, List<V>> byKey) {
        this.byKey = byKey;
    }

    public Map<K, Long> counts() {
        return byKey.entrySet().stream()
                .collect(Collectors.toMap(Map.Entry::getKey, e -> (long) e.getValue().size()));
    }

    public Optional<K> largestKey() {
        return byKey.keySet().stream().max(Comparator.naturalOrder());
    }

    public <R> List<R> flatMap(BiFunction<K, V, R> f) {
        return byKey.entrySet().stream()
                .flatMap(entry -> entry.getValue().stream().map(v -> f.apply(entry.getKey(), v)))
                .collect(Collectors.toList());
    }

    public Runnable deferred(K key) {
        return () -> {
            List<V> values = byKey.get(key);
            if (values == null) {
                return;
            }
            values.forEach(v -> {
                // A lambda inside a lambda, with a comment inside that.
                System.out.println(key + " -> " + v);
            });
        };
    }

    class Inner {
        V firstOf(K key) {
            List<V> values = byKey.get(key);
            return values.isEmpty() ? null : values.get(0);
        }
    }
}
