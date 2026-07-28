package demo;

public class Price {
    public int total(Order order) {
        return order.units() * order.unitPrice() + order.shipping();
    }
}
