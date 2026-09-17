# Language Design

Status: v0 design complete

See [SAO2 v0 Implementation Roadmap](ROADMAP.md) for the high-level build plan.

## Goals

The language should be simple, brief, and powerful. Programs are statically
analysed and transpiled to C. C is an implementation detail rather than part of
the language's interface.

## Types

The initial primitive types are:

| Type | Meaning |
| --- | --- |
| `int` | Signed 64-bit two's-complement integer |
| `float` | 64-bit floating-point number |
| `str` | String |
| `bool` | Boolean |
| `char` | ASCII character |

Container types are written using their literal syntax:

| Type | Meaning |
| --- | --- |
| `[int]` | Vector of integers |
| `{int: str}` | Map from integers to strings |

The forms are generic: `[T]` is a vector of `T`, and `{K: V}` is a map from
`K` to `V`.

Lists are constructed with bracket literals:

```text
[0, 1, 3, 4]
```

Every element must have the list's element type.

Maps are constructed with brace literals containing `key: value` entries:

```text
{k: v, k1: v1}
```

Every key and value must have the map's key and value types respectively.

## Container operations

Lists and maps use indexing for lookup and mutation:

```text
value := items[index];
items[index] = value;
value := table[key];
table[key] = value;
```

Lists grow with `append` and remove elements by index. Maps remove entries by
key. These mutating methods return `()`:

```text
items.append(value);
items.removeIndex(index);
table.removeKey(key);
```

Lists, maps, and strings provide `len()`:

```text
list_length := items.len();
map_length := table.len();
string_length := "hello".len();
```

Negative list and string indices count from the end. An out-of-range index or
missing map key causes a runtime panic. String indexing returns `char`.

Membership uses `in`. For lists it tests values; for maps it tests keys:

```text
has_value := value in items;
has_key := key in table;
```

List iteration follows index order. Map iteration yields keys in insertion
order. Structurally modifying a container during iteration causes a runtime
panic. Container mutation requires a `var` reference.

Characters and strings initially support ASCII only. All strings are immutable
and interned. Equal strings therefore share one canonical runtime value.

## User-defined types

A type with named members is a struct:

```text
type X(a int, b float);
```

A type with unnamed members is a tuple:

```text
type Y(int, float);
```

Members cannot mix the two forms: they must be either all named or all unnamed.
Struct members are accessed by name, such as `value.a`. Tuple members are
accessed by zero-based position, such as `value.0`, `value.1`, and `value.2`.

Structs are objects with reference semantics. Tuples are immutable value types;
their members cannot be modified after construction. Primitive and tuple
members are stored inline. Struct-valued members are also embedded inline by
default. Prefixing a struct member's type with `&` instead stores a
garbage-collected reference:

```text
type Target(x float, y float);
type Body(target Target, sharedTarget &Target);
```

Here `target` is inline and `sharedTarget` is a reference. `&` selects member
storage and does not define a separate first-class reference type. Other
object-valued members are stored as garbage-collected references. A recursive
tuple or inline-struct definition that would have infinite size is rejected;
recursive struct relationships must cross a referenced `&` member or a
packed struct-valued tuple/union representation.

Access syntax is identical for inline and referenced members. Accessing an
inline struct member produces a reference to its stable embedded slot, which
may be passed or returned like any other struct reference. Assigning a struct
to an inline member copies its language-visible fields into that slot while
preserving the slot's identity. Referenced members are rebound instead. Copies
share any referenced objects.

Struct-valued tuple fields and union alternatives use the packed struct
reference representation; they do not embed a complete struct body. A finite
union carrier can therefore carry a reference back to its enclosing struct,
allowing source programs to build cyclic graphs while the inactive union
alternatives remain non-traceable.

### Unions

An untagged union lists its possible types:

```text
type U(int | float | str);
```

A union may instead give each alternative an explicit tag:

```text
type U(A(int) | B(float) | C(float));
```

Every union carries a runtime discriminant. Untagged alternatives must have
distinct types, while tagged alternatives may have identical payload types but
must have distinct tags. A union has at least two alternatives. Alternative
order does not affect type identity, although `Error` must be written last.
Explicitly nested unions retain their nested structure and discriminants; they
are not flattened. An unparenthesized `A | B | C` has three alternatives.

The unit type is written `()` and has exactly one value, also written `()`.
Unit is immutable and has zero-sized storage. It may be stored, passed, placed
in containers and unions, compared for equality, and used as a map key.

## Value construction

Structs are constructed with named members using `=`:

```text
point := Point(x = 10, y = 20);
```

Every struct member must be provided exactly once. Members may appear in any
order, and their expressions are evaluated from left to right. There are no
default member values initially.

Tuples are constructed positionally:

```text
pair := Pair(10, 20.0);
```

The number and types of arguments must match the tuple declaration exactly.

Untagged unions use the union name as their constructor; the argument's type
selects the alternative. Tagged unions qualify the tag with the union name:

```text
u := U(42);
tagged := U.A(42);
```

When an expected union type is known, a value of one of its alternatives is
injected implicitly. `Error(value)` constructs the special error alternative.
`Error` is reserved for this built-in alternative, its constructor, and the
contextual label that selects it in an `is` test or switch arm. User code cannot
declare or bind that name as a type, function, parameter, local, member,
ordinary union tag, loop binding, or in any other identifier position.

Empty list and map literals require an expected type. A postfix type ascription
may disambiguate them when no expected type is otherwise available:

```text
items := [] : [int];
lookup := {} : {int: str};
```

An `if` statement can inspect a union by type or tag:

```text
if value is int: use(value);
if value is A: use(value);
```

Inside the selected body, the tested variable is narrowed directly to the
alternative's payload type.

A `switch` inspects a union without fallthrough:

```text
switch value {
    int: handle_int(value);
    float {
        first(value);
        second();
    }
    str: handle_str(value);
}
```

Each arm names a type or tag, and the tested variable is narrowed directly to
that arm's payload type. `:` introduces a single statement and braces introduce
a block. Every alternative must be covered unless the switch includes an
`else` arm. An `else` arm does not narrow the value. Combined arms are not
initially supported, and a `switch` is a statement rather than an expression.

## Error handling

Errors are represented by unions containing the special `Error` alternative:

```text
fn f() int | Error(str) {
}
```

An operation which can fail but has no other result uses unit as its successful
alternative:

```text
fn check() () | Error(str) {
}
```

`Error` carries a value describing the failure and must always be the last
alternative in a union. Its payload must be exactly one primitive type: `int`,
`float`, `str`, `bool`, or `char`. This restriction applies to named and
anonymous unions. `Error(Type)` is not a standalone result type. The
postfix `?` operator unwraps a successful result or immediately returns its
`Error`, as in Rust. Outside `main`, the enclosing function must return a union
whose top-level `Error` alternative has exactly the same resolved payload type.
There is no widening or union injection between error payloads. `main` is the
exception: using `?` on an error in `main` causes a typed runtime panic which
prints `Error(payload)` using the payload primitive's ordinary formatting,
reports the source location of the `?`, and terminates with a nonzero status.
This is distinct from the explicit `panic(message)` intrinsic, which continues
to require a `str` message.

Postfix `?` removes the top-level `Error` alternative from the expression's
type. If exactly one success alternative remains, the expression has that
alternative's payload type directly. If multiple success alternatives remain,
the expression has their union, preserving the alternatives' tags and explicit
nesting. Consequently, applying `?` to `() | Error(E)` produces `()` on its
successful path.

## Functions

Function parameters and non-unit return types are explicit:

```text
fn add(a int, b int) int {
}

fn noreturn(input int) {
}

fn noargs() {
}

fn doit(var a int) {
}
```

An omitted return type means `()`. Writing `()` explicitly is equivalent.
Normal fallthrough and bare `return;` return the unit value; `return ();` is the
explicit form. Parameters are constant by default. Prefixing a parameter with
`var` permits it to be used to modify its value.

Primitive and tuple parameters are passed by value. Mutating a primitive `var`
parameter or reassigning a tuple `var` parameter changes only the local
parameter. Tuple members remain immutable. Object parameters refer to the
caller's object. Mutating an object through a `var` parameter is therefore
visible to the caller, while rebinding the parameter changes only the local
reference.

Function names are unique and cannot be overloaded. Declaration order does not
matter, and direct and mutual recursion are allowed. Arguments are evaluated
from left to right. Default and variadic parameters are not initially
supported. Every path through a value-returning function must return a value.

Compiler intrinsic names occupy the value namespace and cannot be redeclared
as functions. A type may share a name with a function or intrinsic because type
and value names use separate namespaces. Since both namespaces can contribute
a callable, an unqualified call whose name denotes entries in both is ambiguous
and must be diagnosed; the compiler does not silently prefer the value callable
or the constructor.

The special `Error(value)` union constructor participates in call-target
resolution without ambiguity because `Error` is reserved and cannot be
declared or bound by user code. Qualified tagged construction remains distinct.

The program entry point accepts these equivalent unit and integer-result forms:

```text
fn main() {}
fn main() () {}
fn main() int {}
fn main(args [str]) {}
fn main(args [str]) () {}
fn main(args [str]) int {}
```

Command-line arguments exclude the executable name and retain their original
order. Every argument must be valid ASCII; otherwise the runtime panics before
entering `main`. A unit-returning `main` exits successfully, while an `int`
return becomes the process exit code.

## Runtime built-ins

The runtime provides minimal standard output:

```text
print(value);
println(value);
println();
```

`print` writes without a newline, while `println` appends one. They accept unit,
a primitive value, an immutable tuple recursively containing printable values,
or a union whose every alternative has a recursively printable payload. The
zero-argument `println` writes an empty line. Unit prints as `()`. Strings and
characters print their contents without quotes, integers use decimal, floats
use the following canonical shortest-round-trip representation, and booleans
print as `true` or `false`: the implementation chooses the first significant
digit precision from 1 through 17 for which C-locale `%g` parses back to the
same binary64 value. Exponents use lowercase `e`, omit a positive sign and
redundant leading zeroes; positive and negative zero print as `0` and `-0`.
Thus `1000000.0` prints as `1e6` and `-0.0` prints as `-0`.

Tuples print in constructor form, retaining their nominal name and declaration
order: `Pair(1, true)`. Fields are separated by `, ` and printable nested
values use these same rules without quotes.

Union values retain their constructor form when printed. A named untagged union
prints `Union(payload)`, a named tagged union prints `Union.Tag(payload)`, a
tagged anonymous union prints `Tag(payload)`, and the special alternative prints
`Error(payload)`. An anonymous untagged union prints its active payload without
a wrapper. Nested unions apply these rules recursively, so `() | Error(str)`
prints either `()` or `Error(message)`. An output failure causes a panic. These
operations are compiler intrinsics rather than overloaded or variadic user
functions.

The `panic(message)` intrinsic accepts a `str` and never returns. Standard
input, files, environment variables, clocks, randomness, and process APIs are
not initially provided.

## Panics

Panics represent unrecoverable failures; `Error` represents recoverable
failures. A panic prints its message and source location and terminates the
program with a nonzero exit code. Panics cannot be caught, and there is no stack
unwinding or destructor processing.

## Local variables

Local variables use `:=`, which infers the type from the initializer:

```text
x := 0;     // new constant
var x := 0; // new variable
```

An unqualified declaration creates a constant. The `var` qualifier permits the
variable to be used to modify its value.

For object references, the qualifier applies to the referred value rather than
the reference itself. Constant and `var` references can always be changed to
refer to another object, but only a `var` reference can be used to modify the
referred object.

## Value semantics

`int`, `float`, `bool`, and `char` are inline values and are copied on
assignment and parameter passing. A `str` value is an inline reference to an
immutable interned string.

Tuples are immutable inline values. Assignment copies their members. Object
references within a copied tuple remain shared; tuple copying is not a deep copy.

Structs, lists, and maps are objects. Assignment copies their reference, not the
referred object, and the language performs no implicit deep copies. Aliases
therefore observe mutations made through another mutable reference.

Unions are inline values containing a discriminant and payload. Copying a union
copies its payload according to the payload type: inline values are copied and
object references remain shared.

An unqualified object reference provides read-only access rather than promising
that the object never changes. Mutability is transitive: `var` permits mutation
of the directly referred object and objects reached through its references, as
well as replacement of their fields. Immutable values such as strings and
tuples remain immutable.

Whether escape analysis places an object on the stack or the garbage-collected
heap is not observable by the program.

The generated C may pass tuples through pointers, caller-owned result slots, or
boxed storage to avoid physical copies. These are unobservable implementation
choices and do not change tuple value semantics. The garbage collector traces
any object references contained within tuples.

## Operators

Operators use C syntax and precedence. Arithmetic uses `+`, `-`, `*`, `/`, and
`%`, including unary `+` and `-`. Assignment uses `=`. Compound assignment is
supported with `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, and
`>>=`.

These operations remain subject to the language's static type rules; using C
syntax does not expose C types or other C implementation details.

Comparisons use `==`, `!=`, `<`, `<=`, `>`, and `>=` and always produce a
`bool`. Ordering is defined for `int`, `float`, `char`, and lexicographically
for `str`. Primitive equality compares values. Immutable tuples compare
structurally by their members. Structs, lists, and maps compare reference
identity. Because strings are interned, identity and value equality are
equivalent for strings. Union equality first compares the active discriminant;
values with different active alternatives are unequal, while values with the
same active alternative compare their payloads using that payload type's
equality rule. An inactive union payload is never observed by equality.

Boolean operations use `!`, `&&`, and `||` and require `bool` operands. `&&`
and `||` short-circuit. Bitwise operations use `~`, `&`, `|`, `^`, `<<`, and
`>>` and require `int` operands. Shift counts must be between 0 and 63 inclusive;
other counts cause a runtime panic. Right shift is arithmetic and preserves the
sign bit. Left shift behaves as checked multiplication by a power of two and
panics when the mathematical result is outside the signed 64-bit range.

Numeric conversions are explicit:

```text
float(integer)
int(decimal)
```

Converting a float to an integer truncates toward zero and panics if the result
is outside the integer range. Converting an integer to a float may lose
precision.

Operands are evaluated from left to right. Assignments are statements and do
not produce values. Increment, decrement, comma, and ternary operators are not
initially supported.

Integer arithmetic is checked. Overflow or underflow causes a runtime panic.
Integer division truncates toward zero. Remainder has the same sign as the
dividend and satisfies `a == (a / b) * b + (a % b)`. Integer division or
remainder by zero causes a runtime panic. Both `-2^63 / -1` and `-2^63 % -1`
panic because the corresponding division is not representable. Floating-point
division by zero also causes a runtime panic.
Floating-point values use IEEE 754 binary64 representation. An operation that
produces infinity or NaN causes a runtime panic. Underflow follows IEEE 754
rounding and may produce a subnormal value or zero. Negative zero compares equal
to and has the same hash as positive zero.

## Type semantics

Every named struct, tuple, or union is a distinct nominal type, even when two
declarations have the same shape. There are no implicit conversions between
distinct named types.

Map keys may be `()`, `int`, `str`, `bool`, or immutable tuples composed
recursively only of valid map-key types. Unit has one equality class and a
stable trivial hash; tuple keys use structural equality and hashing. No other
type may be used as a map key.

## Control flow

Braces delimit blocks. Parentheses around conditions are not required, and
conditions must have type `bool`.

```text
if condition {
} else if other {
} else {
}

while condition {
}

for item in items {
}

return value;
break;
continue;
```

Single-statement control-flow bodies may use `:` instead of braces. Braced and
single-statement branches can be mixed:

```text
if condition: xxx;
else if other: yyy;
else {
}
```

Simple statements end with `;`; block statements and function definitions do
not. `while` and `for` are statements and do not produce values. The initial
`for` form supports iteration only, not C-style loops.

Braced blocks may also be used as expressions. The final expression, written
without a semicolon, becomes the block's value:

```text
result := {
    x := calculate();
    x + 1
};
```

A trailing semicolon discards the final expression's value and makes the block
produce `()`. A block without a final expression also produces `()`.

An `if` may be used as either a statement or an expression:

```text
result := if condition {
    first_value
} else {
    second_value
};

result := if condition: first_value else: second_value;
```

An `if` expression requires a final `else`, and every branch must produce a
compatible value. As in Rust, a braced branch produces its final unterminated
expression as its value. A statement-form `if` does not require an `else`.

## Memory management

Memory is managed automatically by a garbage collector. Primitive and tuple
values are conceptually stored inline. Referenceable objects are allocated in
one of two contiguous virtual-address arenas: a heap arena for
garbage-collected lifetimes and a scoped arena for allocations proven not to
outlive the current function. Scoped storage is reclaimed when that function
returns. A compiler may instead use the native C stack when it proves that no
packed reference to the object must be materialized.

On a 64-bit target, the runtime reserves a separate 4-GiB virtual-address range
for each arena without initially committing physical storage for either whole
range. The heap and scoped allocators commit and manage storage independently
within their own ranges. Runtime bookkeeping and storage used by the generated
program or host C implementation outside the arenas are not part of either
limit. A heap allocation first permits the collector to reclaim unreachable
storage; if satisfying it would still exceed the heap arena's capacity, the
program panics. A scoped allocation which would exceed the scoped arena's
capacity also panics.

An object reference is an 8-byte packed struct rather than a native C pointer.
Its representation is equivalent to:

```c
struct ref {
    uint32_t owner_ptr;
    uint32_t member_ptr;
};
```

The all-zero representation is an internal null reference and is not a
language value. Decoded offset zero is permanently reserved in both arenas, so
no allocation or embedded object can produce
`{ owner_ptr: 0, member_ptr: 0 }`. Generated trace code ignores this
representation. The zero discriminant is likewise reserved as the inactive,
non-value state of every runtime union and contains no traceable payload. These
representations allow root storage to be safely zero-initialized before it
contains language values.

Every allocation root is aligned to eight bytes. The low three bits of
`owner_ptr` are therefore available as an arena tag. Tag zero selects the heap
arena, tag one selects the scoped arena, and the remaining tag values are
reserved. The aligned offset with those bits cleared still spans the complete
4-GiB range of either arena.

After removing its tag, `owner_ptr` identifies the root of the complete
enclosing allocation, not merely the referenced object's immediate inline
parent. `member_ptr` is an untagged byte offset identifying the exact referenced
value. It may be unaligned, as for an inline character field. For a reference
to the allocation root, `member_ptr` equals the decoded owner offset. For an
interior reference it instead identifies the stable embedded slot.

Generated C derives temporary raw pointers independently from the two fields.
It selects `heap_base` or `scoped_base` from the `owner_ptr` tag and adds the
decoded owner offset or the unmodified member offset to that same base. Neither
raw pointer is stored as the language reference. Allocation metadata reachable
from the decoded owner records the allocation's size, generated layout,
lifetime class, and collector timestamp. The runtime rejects reserved owner
tags and any individual allocation or arena position that cannot be
represented by the appropriate 32-bit offset.

The compiler performs conservative, context-insensitive escape analysis one
function at a time. For each function it records only the functions called
directly and the functions which call it. Each function tracks its number of
unresolved direct callees. Functions with none are placed in a queue. After a
function is analysed, the compiler updates its callers and queues any whose
unresolved count reaches zero. Every direct call edge is processed once; no
call-path traversal is needed. A summary records which parameters may escape,
including parameters whose value or inline member may be returned.

A caller trusts the callee's summary without reconsidering it based on how the
caller uses the result. Passing an object to a parameter classified as escaping
therefore gives the complete object allocation a garbage-collected lifetime,
even when a more context-sensitive analysis could prove scoped allocation safe.
Functions left unresolved after the queue is exhausted are conservatively
treated as escaping because they are recursive or depend on recursion. Whenever
safety cannot be proven, allocation uses a garbage-collected lifetime.

The collector is non-moving, stop-the-world, and mark-and-sweep. Each heap
allocation records one mark timestamp and participates in the collector's
allocation list. Struct values, including inline subobjects, contain no
collector mark field.

Generated functions use ordinary native C calls. A function with potentially
traceable parameters, locals, or temporaries also has a compiler-generated
shadow-frame struct containing those values. Primitive-only values need not be
members of the shadow frame. Its first member is a common header containing a
link to the caller's active shadow frame and a function-specific traversal
callback. Each invocation zero-initializes its frame, installs the callback,
copies any traceable incoming parameters into it, and links the frame before an
operation can collect; it unlinks the frame on normal return. A function with
no traceable values need not link a frame.

The collector walks the shadow-frame chain rather than inspecting the native C
stack. A frame traversal callback casts the common header to its enclosing
function-specific struct and invokes generated type traversal for each root
field. This handles unions and aggregate values directly and requires neither
a generic offset table nor compiler-generated root push and pop operations.
Tracing all fields for the lifetime of an invocation is correct because unused
fields have a zero-safe representation. It may retain a dead value until the
function returns; clearing dead fields or generating liveness-sensitive
traversal is an optional optimization.

At the start of collection, the runtime advances a global collection timestamp.
A typed root reference resolves its owner from `owner_ptr`, marks that
allocation with the current timestamp when it has a garbage-collected lifetime,
and recursively traces values reachable from the exact object identified by
`member_ptr`. Referencing an outer struct makes its embedded structs reachable;
referencing only an embedded struct does not make its parent or siblings
reachable. Marking the owner retains their shared backing storage, but does not
by itself trace references held by an unreachable parent or sibling.

The trace uses a collection-local visited set keyed by `owner_ptr`, `member_ptr`,
and referenced layout. This prevents cycles without conflating distinct
reachable subobjects in the same allocation. In particular, finding one
interior reference must not prevent the collector from tracing another interior
reference into that allocation.

During sweeping, a garbage-collected allocation is retained when its
allocation-level timestamp was marked during the current collection and is
freed otherwise. `owner_ptr` already identifies the complete enclosing
allocation, so retaining an interior reference requires neither a search for
its root address nor collector metadata in each inline struct.

If a reference to an embedded struct escapes a function, escape analysis places
the complete enclosing allocation under garbage-collected lifetime. Replacing
an embedded struct updates its language-visible fields without changing the
slot's identity or any existing reference to that slot.

## Static analysis

Every expression has a statically known type. The compiler rejects operations,
assignments, arguments, and returns whose types are incompatible.

Container types are checked as part of this analysis. A value of type `[int]`
can contain only integers, and a value of type `{int: str}` can have only
integer keys and string values.

Conditions require `bool`; other values are not implicitly treated as true or
false.

## Initial scope

- A program consists of a single source file and has no modules.
- Only type and function declarations are allowed at the top level; there are
  no top-level variables.
- Functions cannot close over local state; there are initially no closures.
- There is no C foreign-function interface.
- The compiler may emit C, but generated C and C types are not exposed as
  language concepts.

## Grammar

The complete syntax is defined by the [formal EBNF grammar](GRAMMAR.ebnf).
That file also explains the EBNF notation and lists contextual constraints
which are enforced after parsing.

Source files use UTF-8. Identifiers contain ASCII letters, digits, and
underscores, cannot begin with a digit, and are case-sensitive. Keywords are
reserved. Identifiers and character and string contents are initially limited
to ASCII.

Integer literals may be decimal, hexadecimal with `0x`, or binary with `0b`,
and may contain `_` separators. Floating-point literals use decimal notation
with an optional exponent. Boolean literals are `true` and `false`. Character
and string literals use single and double quotes respectively.

```text
42
1_000
0xff
0b1010
1.0
1e10
2.5e-3
true
false
'a'
'\n'
"hello"
```

The supported escapes are `\\`, `\"`, `\'`, `\n`, `\r`, `\t`, `\0`, and
`\xNN`.

Line comments begin with `//`. Block comments use `/*` and `*/` and do not
nest. Whitespace is otherwise insignificant, and newlines do not terminate
statements. Simple statements require `;`, except for the final value expression
of a block. There is no automatic semicolon insertion.

Braces create lexical scopes. A local becomes visible after its initializer.
Local declarations may shadow an earlier local at any time, including within
the same scope; the initializer can therefore refer to the previous binding.
Functions and types are visible throughout the file. Type names and value names
use separate namespaces.

Grammar is interpreted by context: `{key: value}` is a map in expression
position and braces delimit a block in statement position; `|` is a union in
type position and bitwise OR in expression position; `:` separates a map entry
or introduces a single-statement control-flow body, and postfix `: type`
ascribes a type to an empty collection literal.

### Lexical and parsing edge cases

The lexer uses longest-token matching, so `>>=` is recognized before `>>` and
`:=` before `:`. A keyword is recognized only when it forms a complete token.
Comments are recognized only outside literals. Unterminated strings, character
literals, escapes, and block comments are errors. Raw newlines are not allowed
inside string or character literals, and a character literal must contain
exactly one ASCII character after escape decoding.

Numeric signs are operators rather than part of literal tokens. `_` separators
may appear only between digits. A leading zero does not imply octal. A decimal
point requires digits on both sides: `1.0` is a float and `value.0` accesses a
tuple member, while `.5` and `1.` are invalid.

An `else` binds to the nearest unmatched `if`. Assignment is a statement, so
`Point(x = 1)` unambiguously contains a named constructor argument. In type
context, `|` declares a union and `&Type` selects referenced storage for a
struct member. In expression context, `|` and `&` are bitwise operators.

Calls, indexing, member access, and postfix `?` have the highest expression
precedence and may be chained. Braces following `fn`, `if`, `else`, `while`,
`for`, or a braced switch arm always delimit a block. Otherwise, expression
braces are distinguished as follows:

```text
{key: value} // map
{value}      // block expression returning value
{value;}     // block discarding value
{}           // empty map in expression position
```

An expression followed by `;` is a statement which discards its value. An
expression followed directly by `}` supplies its block's value. In an
expression-form single-line `if`, `else` terminates the preceding branch:

```text
value := if condition: 1 else: 2;
```

The intended implementation uses a longest-match lexer, recursive-descent type
and statement parsing, and Pratt expression parsing. Invalid or ambiguous input
produces a syntax error rather than being interpreted automatically.

## Diagnostics and source locations

Every token and syntax-tree node carries a half-open source byte span. The
compiler records line-start offsets and presents one-based line and column
numbers. Tabs expand to four columns when diagnostics are rendered. A
compiler-generated node inherits the span of the source construct that caused
it.

Compile-time source errors and warnings show the source filename, line, column,
relevant source line, and a caret marking the primary span. Related spans may
identify previous declarations or conflicting types. Errors and warnings are
kept in separate collections, each limited independently to 20 entries. Each
collection is sorted stably by source byte position, preserving insertion order
for equal spans. The compiler recovers at safe boundaries such as semicolons
and closing braces to report multiple errors.

In v0, only statically unreachable source produces a warning. Such warnings are
non-fatal: `build` and `run` print them to standard error on both success and
failure, before any fatal diagnostic, without changing the status that the
command would otherwise return. Reachability is structural; the compiler
does not infer non-termination from literal loop conditions or perform general
constant folding for this purpose. The first construct in each contiguous
unreachable region produces one warning. Warning policy beyond unreachable
source is out of scope for v0.

Runtime checks receive a compact location identifier. A generated table maps
each identifier to its SAO2 source location and function. A panic reports the
exact SAO2 location and function without exposing generated C locations.
Runtime stack traces are not initially provided.

Failure to compile generated C is considered a compiler bug. A debug option may
expose the generated C file and compiler diagnostics for investigation.
