/* Checks that every sisu_free and sisu_realloc gets the size and alignment
 * its block was allocated with. Neither ASan nor Valgrind can: the system
 * archive, libsisu_runtime_system.a, frees through Rust's System allocator,
 * which calls plain free(). mimalloc does not check sizes either, so the
 * shim's own table is still needed. Link it with
 * -Wl,--wrap=sisu_alloc,--wrap=sisu_free,--wrap=sisu_realloc (see toolbox.md,
 * "Memory"). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

void *__real_sisu_alloc(size_t size, size_t align);
void __real_sisu_free(void *ptr, size_t size, size_t align);
void *__real_sisu_realloc(void *ptr, size_t old_size, size_t align, size_t new_size);

/* Live blocks in an open-addressed table keyed by pointer, so a lookup stays
 * constant time with millions of blocks live. sisu_alloc aborts on size 0, so
 * no two live blocks share a pointer. */
enum { BITS = 22, SLOTS = 1 << BITS, MASK = SLOTS - 1, MAX_LIVE = SLOTS / 4 * 3 };
static struct block { void *ptr; size_t size, align; } table[SLOTS];
static size_t count;

static size_t home(void *ptr) {
    return (size_t)(((uint64_t)(uintptr_t)ptr * 0x9E3779B97F4A7C15u) >> (64 - BITS));
}

/* The slot holding ptr, or the empty slot where it would go. */
static size_t find(void *ptr) {
    size_t i = home(ptr);
    while (table[i].ptr && table[i].ptr != ptr)
        i = (i + 1) & MASK;
    return i;
}

static void insert(void *ptr, size_t size, size_t align) {
    if (count == MAX_LIVE) {
        fprintf(stderr, "layout-shim: more than %d live blocks\n", MAX_LIVE);
        abort();
    }
    table[find(ptr)] = (struct block){ptr, size, align};
    count++;
}

/* Empties slot i, moving later entries of its probe run back so lookups still find them. */
static void erase(size_t i) {
    for (size_t j = i;;) {
        j = (j + 1) & MASK;
        if (!table[j].ptr)
            break;
        if (((j - i) & MASK) <= ((j - home(table[j].ptr)) & MASK)) {
            table[i] = table[j];
            i = j;
        }
    }
    table[i].ptr = NULL;
    count--;
}

/* The slot of a live block, after checking the layout the caller passed for it. */
static size_t checked(void *ptr, size_t size, size_t align, const char *verb) {
    size_t i = find(ptr);
    if (!table[i].ptr) {
        fprintf(stderr, "layout-shim: %s of a block sisu_alloc did not return: %p\n", verb, ptr);
        abort();
    }
    if (table[i].size != size || table[i].align != align) {
        fprintf(stderr, "layout-shim: allocated %zu/%zu, %s as %zu/%zu\n", table[i].size,
                table[i].align, verb, size, align);
        abort();
    }
    return i;
}

void *__wrap_sisu_alloc(size_t size, size_t align) {
    void *ptr = __real_sisu_alloc(size, align);
    insert(ptr, size, align);
    return ptr;
}

void __wrap_sisu_free(void *ptr, size_t size, size_t align) {
    erase(checked(ptr, size, align, "freed"));
    __real_sisu_free(ptr, size, align);
}

void *__wrap_sisu_realloc(void *ptr, size_t old_size, size_t align, size_t new_size) {
    erase(checked(ptr, old_size, align, "reallocated"));
    void *grown = __real_sisu_realloc(ptr, old_size, align, new_size);
    insert(grown, new_size, align);
    return grown;
}
