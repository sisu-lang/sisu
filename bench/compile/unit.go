// One copy of the program that scripts/compile_speed.py repeats. The generator puts the
// number of the copy in each name that ends in the placeholder.
// unit.sisu, unit.rs and unit.cpp hold the same program.

package main

import "fmt"

// Ord is the ordering that the generic function asks for. Go has no built-in one.
type Ord interface {
	~int64 | ~string | ~uint8
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
	label := fmt.Sprintf("tally %d/%d", tally.count, tally.sum)
	return total + int64(len(label))
}
