# API contracts

An API is a compatibility boundary between a producer and its consumers. A contract defines one part of that boundary; contracts in the same API share one compatibility version.

## Representation

Each API directory contains an `api.yaml`, validated by [schema.json](schema.json). The index records the stable `API-NNNN` ID, the name, the compatibility version and the contracts. Each contract records a globally unique, stable `CON-NNNN` ID, a name, a Markdown document, an optional structural JSON schema and the requirements it specifies. IDs are never reused. Document and schema paths are relative to the index, stay inside the directory and are not reached through symbolic links.

## Authority

Requirements state the product promises; contracts define their exact interfaces. A structural schema defines fields, types and representable constraints. Its Markdown document defines meaning, sequencing, errors and conditions that need validation beyond the schema. Do not repeat field tables or bounds in both places.

Contracts and requirements must agree. Resolve contradictions with the owner; existing code never silently changes a contract.
