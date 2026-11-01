// One copy of the program that scripts/compile_speed.py repeats. The generator
// puts the number of the copy in each name that ends in the placeholder.
// unit.sisu, unit.rs and unit.go hold the same program.
// No header is included: a standard header would add the library to what the
// compiler parses.
extern "C" int printf(const char *, ...);

// A string of at most 63 bytes. Every string of the program is shorter.
struct Str {
  char b[64];
  int n;
  Str() : n(0) {}
  Str(const char *s) : n(0) {
    while (s[n]) {
      b[n] = s[n];
      n++;
    }
  }
  long long len() const { return n; }
};

Str operator+(Str a, Str c) {
  for (int i = 0; i < c.n; i++)
    a.b[a.n++] = c.b[i];
  return a;
}

bool operator<(const Str &a, const Str &c) {
  int m = a.n < c.n ? a.n : c.n;
  for (int i = 0; i < m; i++)
    if (a.b[i] != c.b[i])
      return a.b[i] < c.b[i];
  return a.n < c.n;
}

// The decimal text of a number that is not negative.
Str str(long long v) {
  char t[24];
  int k = 0;
  do {
    t[k++] = (char)('0' + v % 10);
    v /= 10;
  } while (v > 0);
  Str s;
  while (k > 0)
    s.b[s.n++] = t[--k];
  return s;
}

Str str(Str s) { return s; }

template <class T> struct Opt {
  bool has = false;
  T val{};
};

template <class T> Opt<T> some(T v) {
  Opt<T> o;
  o.has = true;
  o.val = v;
  return o;
}

template <class T>
concept Ord = requires(T a, T b) { a < b; };

template <class T> struct NodeXX {
  T value;
  NodeXX<T> *next;
};

template <class T> struct StackXX {
  NodeXX<T> *top = nullptr;
  long long size = 0;

  void push(T x) {
    top = new NodeXX<T>{x, top};
    size++;
  }

  Opt<T> pop() {
    if (!top)
      return Opt<T>{};
    NodeXX<T> *node = top;
    top = node->next;
    size--;
    T v = node->value;
    delete node;
    return some(v);
  }
};

template <class U> struct SourceXX {
  using Unit = U;
  virtual Opt<U> next() = 0;
  virtual ~SourceXX() {}
};

struct CountdownXX : SourceXX<long long> {
  long long left;
  CountdownXX(long long l) : left(l) {}
  Opt<long long> next() override {
    if (left == 0)
      return Opt<long long>{};
    left--;
    return some(left + 1);
  }
};

struct LettersXX : SourceXX<Str> {
  long long taken;
  long long limit;
  LettersXX(long long t, long long l) : taken(t), limit(l) {}
  Opt<Str> next() override {
    if (taken == limit)
      return Opt<Str>{};
    taken++;
    return some(Str("x") + str(taken));
  }
};

template <class S> long long drainXX(S &s) {
  long long total = 0;
  for (Opt<typename S::Unit> x = s.next(); x.has; x = s.next())
    total = total * 3 + str(x.val).len();
  return total;
}

template <Ord T, int N> Opt<T> largestXX(T (&xs)[N]) {
  Opt<T> best;
  for (int i = 0; i < N; i++)
    if (!best.has || best.val < xs[i])
      best = some(xs[i]);
  return best;
}

struct ShapeXX {
  enum Kind { Dot, Line, Rect } kind;
  long long a;
  long long b;
};

long long areaXX(ShapeXX s) {
  switch (s.kind) {
  case ShapeXX::Dot:
    return 1;
  case ShapeXX::Line:
    return s.a;
  case ShapeXX::Rect:
    return s.a * s.b;
  }
  return 0;
}

struct TallyXX {
  long long count = 0;
  long long sum = 0;

  void add(long long n) {
    count++;
    sum += n;
  }

  Opt<long long> mean() {
    if (count == 0)
      return Opt<long long>{};
    return some(sum / count);
  }
};

long long runXX() {
  StackXX<long long> ints;
  StackXX<Str> words;
  for (long long i = 0; i < 4; i++) {
    ints.push(i * 3 + 1);
    words.push(Str("w") + str(i));
  }
  long long total = 0;
  for (Opt<long long> v = ints.pop(); v.has; v = ints.pop())
    total = total * 2 + v.val;
  for (Opt<Str> w = words.pop(); w.has; w = words.pop())
    total += w.val.len();
  total += ints.size + words.size;
  CountdownXX countdown(5);
  total += drainXX(countdown);
  LettersXX letters(0, 4);
  total += drainXX(letters);
  long long nums[] = {3, 9, 2};
  total += largestXX(nums).val;
  Str strs[] = {"b", "ccc", "a"};
  total += largestXX(strs).val.len();
  unsigned char bytes[] = {4, 8, 1};
  total += (long long)largestXX(bytes).val;
  TallyXX tally;
  ShapeXX shapes[] = {
      {ShapeXX::Dot, 0, 0}, {ShapeXX::Line, 4, 0}, {ShapeXX::Rect, 3, 5}};
  for (ShapeXX s : shapes)
    tally.add(areaXX(s));
  total += tally.mean().val;
  Str label = Str("tally ") + str(tally.count) + Str("/") + str(tally.sum);
  return total + label.len();
}
