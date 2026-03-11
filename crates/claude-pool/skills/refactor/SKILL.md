---
name: refactor
description: Refactor code toward a specific goal.
metadata:
  arguments:
    - name: target
      description: Code or file path to refactor.
      required: true
    - name: goal
      description: What the refactoring should achieve.
      required: true
---

Refactor the following code. Goal: {goal}

{target}
