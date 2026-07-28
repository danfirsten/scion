export class Basket {
    private items: string[] = [];

    add(item: string): void {
        this.items.push(item);
    }

    size(): number {
        return this.items.length;
    }
}
