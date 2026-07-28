package demo;

import java.util.List;
import java.util.Map;

public class Orders {
    private final Map<String, Order> byId;

    public Order find(String id) {
        return byId.get(id);
    }

    public int total(Order order) {
        return order.units() * order.unitPrice();
    }

    public void drop(String id) {
        byId.remove(id);
    }
}
