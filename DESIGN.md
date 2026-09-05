# Language Design

Status: v0 design complete

## Goals

The language should be simple, brief, and powerful. Programs are statically
analysed and transpiled to C. C is an implementation detail rather than part of
the language's interface.

## Types

The initial primitive types are:

| Type | Meaning |
| --- | --- |
| `int` | 64-bit integer |
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
key. The removal methods do not produce values:

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
members are stored inline. Object-valued members are stored as garbage-collected
references rather than embedded inline. A recursive tuple definition that would
have infinite inline size is rejected.

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

`Error` carries a value describing the failure and must always be the last
alternative in a union. The postfix `?` operator unwraps a successful result or
immediately returns its `Error`, as in Rust. The enclosing function must return
a union with a compatible `Error` alternative. `main` is the exception: using
`?` on an error in `main` causes a runtime panic.

## Functions

Function parameters and non-empty return types are explicit:

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

The return type is omitted when a function returns no value. Parameters are
constant by default. Prefixing a parameter with `var` permits it to be used to
modify its value.

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

The program entry point is `fn main()`. It may omit its return type or return an
`int`, which becomes the process exit code.

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
that the object never changes. Mutability is shallow: `var` permits mutation of
the directly referred object and replacement of its fields, but does not grant
mutable access to separate objects referenced by those fields.

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
equivalent for strings.

Boolean operations use `!`, `&&`, and `||` and require `bool` operands. `&&`
and `||` short-circuit. Bitwise operations use `~`, `&`, `|`, `^`, `<<`, and
`>>` and require `int` operands. A negative shift count or one greater than 63
causes a runtime panic. Overflow from a left shift also causes a runtime panic.

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
Integer or floating-point division by zero also causes a runtime panic.
Floating-point values use IEEE 754 binary64 representation. An operation that
produces infinity or NaN causes a runtime panic. Underflow follows IEEE 754
rounding and may produce a subnormal value or zero. Negative zero compares equal
to and has the same hash as positive zero.

## Type semantics

Every named struct, tuple, or union is a distinct nominal type, even when two
declarations have the same shape. There are no implicit conversions between
distinct named types.

Map keys may be `int`, `str`, `bool`, or immutable tuples composed recursively
only of valid map-key types. Tuple keys use structural equality and hashing.
No other type may be used as a map key.

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

A trailing semicolon discards the final expression's value. A block without a
value cannot be used where a value is required.

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
values are conceptually stored inline. Objects are allocated on the
garbage-collected heap unless escape analysis proves that they do not outlive
the current function, in which case they are allocated on the stack.

The compiler performs whole-program escape analysis. Functions are analysed
from the leaves of the call graph upward. Recursive groups are analysed together
until their escape information reaches a fixed point. When safety cannot be
proven, the object is allocated on the garbage-collected heap.

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

## Remaining implementation specification

The core language design is complete. Implementation work must make the
following details precise:

- Formal EBNF grammar
- Lexer and parser edge cases
- Exact signed integer division, remainder, and shift behaviour
- Minimal runtime built-ins, including output and command-line arguments
- Garbage collector implementation details
- Diagnostics and source locations
