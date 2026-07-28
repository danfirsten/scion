export class Basket {
    private items: string[] = [];

    count(): number {
        return this.items.length;
    }

    add(item: string): void {
        if (item.length > 0) {
            this.items.push(item);
        }
    }
}
