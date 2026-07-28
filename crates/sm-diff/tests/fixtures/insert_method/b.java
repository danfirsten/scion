package demo;

public class Cache {
    public String get(String key) {
        return store.get(key);
    }

    public void put(String key, String value) {
        store.put(key, value);
    }
}
