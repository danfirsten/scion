package omega;

public interface Visitor<R> {
    R visitLeaf(Leaf leaf);

    R visitBranch(Branch branch);
}
