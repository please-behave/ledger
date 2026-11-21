# Requirements

A requirement is one binding, verifiable statement about what the ledger must do or be. The [schema](schema.json) defines its fields.

- **One statement.** It states one condition that can be verified on its own.
- **Observable.** It describes behavior or a property visible from outside, not a function, a type or a file.
- **Significant.** It stays true after a refactoring and its reason fits into one sentence, which is the `rationale`.
- **Code-free.** It never names code or tests. Tests name the requirements they verify.
- **Stated once.** A fact that another source defines is referred to, not copied.

A change in behavior starts with the requirement. Update it in the same commit as the code, tests and documentation. A requirement that no longer holds is marked obsolete or deleted, never left in place and ignored.

Cite a decision in `decisions` only when a significant choice with a real alternative stands behind the requirement. A decision never names requirements.
