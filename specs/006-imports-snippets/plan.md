# Implementation Plan: Session Imports and Snippets

Pure parsers in `linecon-config::import`, producing a common record applied to the tree by one function.
The Windows registry is read through `reg export`, which reuses the .reg parser and adds no dependency.
Snippets are stored on folders and at the root.
