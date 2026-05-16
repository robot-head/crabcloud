# Working agreements for this repo

## Execution mode

**Always use subagent-driven development (`superpowers:subagent-driven-development`) when executing implementation plans.** Inline execution is not the default.

**Batch tasks when possible.** Dispatch parallel subagents for independent tasks per `superpowers:dispatching-parallel-agents`. Sequential per-task dispatch is the fallback when tasks share state or depend on each other.

The two-stage review (implementer report → spec-compliance review) stays mandatory.
