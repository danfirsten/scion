package demo;

public class Repo {
    public Item load(String id) {
        return store.get(id);
    }

    public void save(Item item) {
        validate(item);
        store.put(item.id(), item);
        audit.record(item);
    }
}
