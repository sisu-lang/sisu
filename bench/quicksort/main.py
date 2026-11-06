def random_array(n):
    xs = [0] * n
    x = 42
    for i in range(n):
        x = (x * 1103515245 + 12345) % 2147483648
        xs[i] = x
    return xs


def quicksort(a, lo, hi) -> None:
    if lo >= hi:
        return
    pivot = a[hi]
    i = lo
    for j in range(lo, hi):
        if a[j] < pivot:
            a[i], a[j] = a[j], a[i]
            i += 1
    a[hi] = a[i]
    a[i] = pivot
    quicksort(a, lo, i - 1)
    quicksort(a, i + 1, hi)


n = 10_000_000
xs = random_array(n)
quicksort(xs, 0, n - 1)
print(sum(x * (i % 100) for i, x in enumerate(xs)))
