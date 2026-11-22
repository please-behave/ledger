# Canonical Rule Digest

The ledger owns this format. Producers of registrations and results use it to
identify the same rule version metadata under why:DEC-0012.

## Input

| Field | Value |
|---|---|
| `source` | Normalized Git repository identity defined by why:DEC-0010, as a string. |
| `path` | Repository-relative path of the rule directory, as a string. |
| `id` | Source-local rule ID, as a string. |
| `version` | Owner-assigned rule version defined by why:DEC-0011, as a string. |
| `kind` | The string `check` or `review`. |
| `name` | Display name as a string, when specified. |
| `description` | Rule description as a string, when specified. |
| `blocking` | Effective blocking boolean, required for a review after its default is resolved. |
| `prompt` | Contents of the review prompt as a string before any run-specific parameter substitution, required for a review. |
| `knowledge` | Complete [rule knowledge document](https://github.com/please-behave/cli/tree/main/why/contracts/rule-knowledge) as an object, when present. |

Omit absent optional members and omit `blocking` and `prompt` for checks.
An empty string is a value distinct from absence. Include every member of a
present knowledge document, including members unfamiliar to the digest producer.
The caller supplies validated identity and version values and projects the
covered fields into this object. The canonical form includes only these
top-level members. When decoding a serialized input, the caller must reject
duplicate keys before converting it to an object.

## Canonical form

1. Accept only Unicode scalar strings, booleans, arrays and objects with string
   keys. Reject numbers and nulls.
2. In every string, including object keys, replace each CRLF pair with LF and
   then replace each remaining CR with LF. Reject any object whose keys become
   duplicates after this replacement. Preserve all other code points.
3. Sort each object's members by the unsigned UTF-8 bytes of the normalized,
   unescaped keys in ascending lexicographic order, with a shorter prefix
   first. Preserve array order.
4. Encode as UTF-8 JSON without a byte-order mark, insignificant whitespace or
   a trailing newline. Write the booleans as `true` and `false`. In strings,
   escape quotation mark and reverse solidus as `\"` and `\\`. Escape
   U+0008, U+0009, U+000A, U+000C and U+000D as `\b`, `\t`, `\n`, `\f`
   and `\r`, respectively. Escape all remaining code points U+0000 through
   U+001F as `\u00xx` with lowercase hexadecimal digits. Write every other
   code point directly as UTF-8, including solidus and non-ASCII characters.

Hash the resulting bytes with SHA-256 and encode the 32-byte result as 64
lowercase hexadecimal digits preceded by `v1:sha256:`. The prefix identifies
the canonicalization scheme version and the hash algorithm.

## Normative test vectors

[Vectors](vectors.json) supplies an array of named cases with `input` objects
and their expected `digest` strings. The two review cases have equivalent
inputs and must produce identical digests.
