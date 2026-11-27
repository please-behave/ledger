# Components

A component is a cohesive part of the compiled product with one responsibility. Every file below `src` and every file the sources embed at compile time, such as contract schemas and migrations, belongs to exactly one component. The [Why standard](https://github.com/please-behave/why/tree/main/standard) defines the structure.

- **One responsibility.** The name states what the component is responsible for. Split a component when its scope needs "and" between unrelated matters.
- **Files, exactly.** `files` lists every file by its path from the repository root, without wildcards or directories.
- **A view of the implementation.** A component names the requirements it implements and the components it depends on. `depends_on` equals the dependencies found in the source. Requirements never name components.
- **Boundaries.** `out_of_scope` names the owner of a neighboring matter, and `must_not` lists prohibitions that hold even when a task seems to call for them.

Tests are not components. A change that adds, moves or removes a compiled file, or adds a dependency between components, updates the component in the same commit. The `component-consistency` rule checks this.
