---
name: write_tests
description: Generate tests for existing code.
argument-hint: "<file|module|code>"
metadata:
  arguments:
    - name: target
      description: File path, module, or code to test.
      required: true
---

Write comprehensive tests for the following code. Cover edge cases and error paths.

{target}
