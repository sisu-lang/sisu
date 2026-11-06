class Node:
    def __init__(self, left: "Node | None", right: "Node | None") -> None:
        self.left = left
        self.right = right

    def check(self) -> int:
        return (
            1 + (self.left.check() if self.left else 0) + (self.right.check() if self.right else 0)
        )


def make(depth: int) -> Node:
    if depth == 0:
        return Node(None, None)
    return Node(make(depth - 1), make(depth - 1))


min_depth = 4
max_depth = 18
print(make(max_depth + 1).check())
long_lived = make(max_depth)
for depth in range(min_depth, max_depth + 1, 2):
    iterations = 1 << (max_depth - depth + min_depth)
    print(iterations)
    print(sum(make(depth).check() for _ in range(iterations)))
print(long_lived.check())
