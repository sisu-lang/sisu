// One copy of the program that scripts/compile_speed.py repeats. The generator puts the
// number of the copy in each name that ends in the placeholder. It also puts the number in
// one string, so that rustc cannot merge the `run` functions of the copies.
// unit.sisu, unit.rs and unit.cpp hold the same program.

package main

import "fmt"

// Ord is the ordering that the generic function asks for. Go has no built-in one.
type Ord interface {
	~int64 | ~string | ~uint8
}

// A value of 100 `int64` fields: 800 bytes. `wide` builds it, copies it, writes a field of the
// copy, reads every field and compares the two.
type Wide struct {
	f0  int64
	f1  int64
	f2  int64
	f3  int64
	f4  int64
	f5  int64
	f6  int64
	f7  int64
	f8  int64
	f9  int64
	f10 int64
	f11 int64
	f12 int64
	f13 int64
	f14 int64
	f15 int64
	f16 int64
	f17 int64
	f18 int64
	f19 int64
	f20 int64
	f21 int64
	f22 int64
	f23 int64
	f24 int64
	f25 int64
	f26 int64
	f27 int64
	f28 int64
	f29 int64
	f30 int64
	f31 int64
	f32 int64
	f33 int64
	f34 int64
	f35 int64
	f36 int64
	f37 int64
	f38 int64
	f39 int64
	f40 int64
	f41 int64
	f42 int64
	f43 int64
	f44 int64
	f45 int64
	f46 int64
	f47 int64
	f48 int64
	f49 int64
	f50 int64
	f51 int64
	f52 int64
	f53 int64
	f54 int64
	f55 int64
	f56 int64
	f57 int64
	f58 int64
	f59 int64
	f60 int64
	f61 int64
	f62 int64
	f63 int64
	f64 int64
	f65 int64
	f66 int64
	f67 int64
	f68 int64
	f69 int64
	f70 int64
	f71 int64
	f72 int64
	f73 int64
	f74 int64
	f75 int64
	f76 int64
	f77 int64
	f78 int64
	f79 int64
	f80 int64
	f81 int64
	f82 int64
	f83 int64
	f84 int64
	f85 int64
	f86 int64
	f87 int64
	f88 int64
	f89 int64
	f90 int64
	f91 int64
	f92 int64
	f93 int64
	f94 int64
	f95 int64
	f96 int64
	f97 int64
	f98 int64
	f99 int64
}

func wide(base int64) int64 {
	w := Wide{
		f0:  base,
		f1:  base + 1,
		f2:  base + 2,
		f3:  base + 3,
		f4:  base + 4,
		f5:  base + 5,
		f6:  base + 6,
		f7:  base + 7,
		f8:  base + 8,
		f9:  base + 9,
		f10: base + 10,
		f11: base + 11,
		f12: base + 12,
		f13: base + 13,
		f14: base + 14,
		f15: base + 15,
		f16: base + 16,
		f17: base + 17,
		f18: base + 18,
		f19: base + 19,
		f20: base + 20,
		f21: base + 21,
		f22: base + 22,
		f23: base + 23,
		f24: base + 24,
		f25: base + 25,
		f26: base + 26,
		f27: base + 27,
		f28: base + 28,
		f29: base + 29,
		f30: base + 30,
		f31: base + 31,
		f32: base + 32,
		f33: base + 33,
		f34: base + 34,
		f35: base + 35,
		f36: base + 36,
		f37: base + 37,
		f38: base + 38,
		f39: base + 39,
		f40: base + 40,
		f41: base + 41,
		f42: base + 42,
		f43: base + 43,
		f44: base + 44,
		f45: base + 45,
		f46: base + 46,
		f47: base + 47,
		f48: base + 48,
		f49: base + 49,
		f50: base + 50,
		f51: base + 51,
		f52: base + 52,
		f53: base + 53,
		f54: base + 54,
		f55: base + 55,
		f56: base + 56,
		f57: base + 57,
		f58: base + 58,
		f59: base + 59,
		f60: base + 60,
		f61: base + 61,
		f62: base + 62,
		f63: base + 63,
		f64: base + 64,
		f65: base + 65,
		f66: base + 66,
		f67: base + 67,
		f68: base + 68,
		f69: base + 69,
		f70: base + 70,
		f71: base + 71,
		f72: base + 72,
		f73: base + 73,
		f74: base + 74,
		f75: base + 75,
		f76: base + 76,
		f77: base + 77,
		f78: base + 78,
		f79: base + 79,
		f80: base + 80,
		f81: base + 81,
		f82: base + 82,
		f83: base + 83,
		f84: base + 84,
		f85: base + 85,
		f86: base + 86,
		f87: base + 87,
		f88: base + 88,
		f89: base + 89,
		f90: base + 90,
		f91: base + 91,
		f92: base + 92,
		f93: base + 93,
		f94: base + 94,
		f95: base + 95,
		f96: base + 96,
		f97: base + 97,
		f98: base + 98,
		f99: base + 99,
	}
	before := w
	w.f0 += 1
	var total int64
	total += w.f0
	total += w.f1
	total += w.f2
	total += w.f3
	total += w.f4
	total += w.f5
	total += w.f6
	total += w.f7
	total += w.f8
	total += w.f9
	total += w.f10
	total += w.f11
	total += w.f12
	total += w.f13
	total += w.f14
	total += w.f15
	total += w.f16
	total += w.f17
	total += w.f18
	total += w.f19
	total += w.f20
	total += w.f21
	total += w.f22
	total += w.f23
	total += w.f24
	total += w.f25
	total += w.f26
	total += w.f27
	total += w.f28
	total += w.f29
	total += w.f30
	total += w.f31
	total += w.f32
	total += w.f33
	total += w.f34
	total += w.f35
	total += w.f36
	total += w.f37
	total += w.f38
	total += w.f39
	total += w.f40
	total += w.f41
	total += w.f42
	total += w.f43
	total += w.f44
	total += w.f45
	total += w.f46
	total += w.f47
	total += w.f48
	total += w.f49
	total += w.f50
	total += w.f51
	total += w.f52
	total += w.f53
	total += w.f54
	total += w.f55
	total += w.f56
	total += w.f57
	total += w.f58
	total += w.f59
	total += w.f60
	total += w.f61
	total += w.f62
	total += w.f63
	total += w.f64
	total += w.f65
	total += w.f66
	total += w.f67
	total += w.f68
	total += w.f69
	total += w.f70
	total += w.f71
	total += w.f72
	total += w.f73
	total += w.f74
	total += w.f75
	total += w.f76
	total += w.f77
	total += w.f78
	total += w.f79
	total += w.f80
	total += w.f81
	total += w.f82
	total += w.f83
	total += w.f84
	total += w.f85
	total += w.f86
	total += w.f87
	total += w.f88
	total += w.f89
	total += w.f90
	total += w.f91
	total += w.f92
	total += w.f93
	total += w.f94
	total += w.f95
	total += w.f96
	total += w.f97
	total += w.f98
	total += w.f99
	if w != before {
		total += 2
	}
	return total
}

type NodeXX[T any] struct {
	value T
	next  *NodeXX[T]
}

type StackXX[T any] struct {
	top  *NodeXX[T]
	size int64
}

func (s *StackXX[T]) push(x T) {
	s.top = &NodeXX[T]{x, s.top}
	s.size++
}

func (s *StackXX[T]) pop() (T, bool) {
	if s.top == nil {
		var zero T
		return zero, false
	}
	node := s.top
	s.top = node.next
	s.size--
	return node.value, true
}

// Go has no associated types: the item type is a type parameter of the interface.
type SourceXX[T any] interface {
	next() (T, bool)
}

type CountdownXX struct {
	left int64
}

func (c *CountdownXX) next() (int64, bool) {
	if c.left == 0 {
		return 0, false
	}
	c.left--
	return c.left + 1, true
}

type LettersXX struct {
	taken int64
	limit int64
}

func (l *LettersXX) next() (string, bool) {
	if l.taken == l.limit {
		return "", false
	}
	l.taken++
	return fmt.Sprintf("x%d", l.taken), true
}

func drainXX[T any, S SourceXX[T]](s S) int64 {
	var total int64
	for x, ok := s.next(); ok; x, ok = s.next() {
		total = total*3 + int64(len(fmt.Sprint(x)))
	}
	return total
}

func largestXX[T Ord](xs []T) (T, bool) {
	var best T
	found := false
	for _, x := range xs {
		if !found || best < x {
			best, found = x, true
		}
	}
	return best, found
}

type ShapeXX interface {
	isShapeXX()
}

type DotXX struct{}

type LineXX struct {
	n int64
}

type RectXX struct {
	w int64
	h int64
}

func (DotXX) isShapeXX()  {}
func (LineXX) isShapeXX() {}
func (RectXX) isShapeXX() {}

func areaXX(s ShapeXX) int64 {
	switch v := s.(type) {
	case DotXX:
		return 1
	case LineXX:
		return v.n
	case RectXX:
		return v.w * v.h
	}
	return 0
}

type TallyXX struct {
	count int64
	sum   int64
}

func (t *TallyXX) add(n int64) {
	t.count++
	t.sum += n
}

func (t *TallyXX) mean() (int64, bool) {
	if t.count == 0 {
		return 0, false
	}
	return t.sum / t.count, true
}

func runXX() int64 {
	ints := &StackXX[int64]{}
	words := &StackXX[string]{}
	for i := int64(0); i < 4; i++ {
		ints.push(i*3 + 1)
		words.push(fmt.Sprintf("w%d", i))
	}
	var total int64
	for v, ok := ints.pop(); ok; v, ok = ints.pop() {
		total = total*2 + v
	}
	for w, ok := words.pop(); ok; w, ok = words.pop() {
		total += int64(len(w))
	}
	total += ints.size + words.size
	total += drainXX(&CountdownXX{left: 5})
	total += drainXX(&LettersXX{taken: 0, limit: 4})
	biggest, _ := largestXX([]int64{3, 9, 2})
	total += biggest
	longest, _ := largestXX([]string{"b", "ccc", "a"})
	total += int64(len(longest))
	small, _ := largestXX([]uint8{4, 8, 1})
	total += int64(small)
	tally := &TallyXX{}
	shapes := []ShapeXX{DotXX{}, LineXX{4}, RectXX{3, 5}}
	for _, s := range shapes {
		tally.add(areaXX(s))
	}
	mean, _ := tally.mean()
	total += mean
	total += wide(XX)
	label := fmt.Sprintf("tally XX %d/%d", tally.count, tally.sum)
	return total + int64(len(label))
}
