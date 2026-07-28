package demo;

public class Price {
    public int total(Order order) {
        int goods = order.units() * order.unitPrice();
        return goods + order.shipping();
    }
}
