package demo;

public class Cache {
    public String get(String key) {
        return store.get(key);
    }

    public void clear() {
        store.clear();
    }
}
