# Implementation Plan: Session Tree and Inheritance

New crate `linecon-config`, with no UI or engine dependency beyond the algorithm and settings
types. `Overrides` holds one `Option` per inheritable field; a macro generates the matching
`Resolved` struct so a new field cannot forget its inheritance or provenance.

Constitution: satisfies V (visible inheritance, unit tested) and VI (no secrets in config).
Portable mode implements III.
