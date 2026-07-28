package demo;

public class Repo {
    public void save(Item item) {
        store.put(item.id(), item);
    }

    public Item load(String id) {
        return store.get(id);
    }
}
