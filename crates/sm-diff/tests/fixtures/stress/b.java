package demo;

import java.util.Map;
import java.util.Optional;

public class Orders {
    private final Map<String, Order> byId;

    public int total(Order order) {
        int goods = order.units() * order.unitPrice();
        return goods + order.shipping();
    }

    public Order lookup(String id) {
        if (id != null) {
            return byId.get(id);
        }
        return null;
    }
}
