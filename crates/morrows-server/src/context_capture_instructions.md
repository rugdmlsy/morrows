Context preparation and capture:

Preserve execution inputs:
- Preserve the inputs that a successor needs.
- Do not preserve only a narrative conclusion.
- Record the machine and absolute workspace for operational work.
- Record the source branch or revision.
- Record report and receipt locations.
- Record reproduction entry points and known parameters.
- Record unresolved inputs.
- Preserve useful existing background.
- Register resolvable evidence references as artifacts when authorized.
- Do not replace original evidence with an unsupported summary.

Record provenance:
- Attach a source reference to imported facts.
- Record the observation time.
- Record verification status and limitations.
- A historical path does not prove that a file still exists.
- A historical command does not prove that its intended result succeeded.
- Command output can be truncated.
- Compare old summaries with newer activity before treating them as current.
- Do not overwrite newer evidence with an older summary.
- Keep unknowns and conflicting observations explicit.

Use authorized sources:
- An external Provider Session ID is not authorization to read private provider history.
- Do not automatically follow private provider references.
- Do not copy credentials or private provider conversations into shared Task knowledge.
- Capture only authorized facts relevant to the Task.
- Preserve provenance for those facts.
- Record missing access or missing inputs as gaps.
- Record the next retrieval step.
- Do not invent paths, commands, or verification results.

Prepare before substantive work:
- Read task_context for a scoped Task.
- Read project_get when you only need Project knowledge.
- Follow the relevant paging tools.
- For executor intake, complete every required task_intake page.
- Then complete the multi-turn Human Interview.
- Use Task collaboration for persisted Human and Agent messages.
- The provider conversation is private runtime state unless you persist facts to Morrows.
- Read current Project or organization knowledge relevant to the goal.
- Do not assume that the first page contains all relevant knowledge.
- Resolve recorded inputs in the current environment.
- Check inputs before you repeat historical commands.
- After a context update, read task_context again.
- Ensure goals, constraints, execution references, and next steps remain available.
- Use context_package_assemble when you need an immutable package.
- Context assembly cannot recover information Morrows never persisted.

Continue interrupted work:
- Prefer the predecessor Run's latest immutable milestone when available.
- Reconcile its ContextRevision with the current Task.
- Reconcile its evidence with the live environment.
- Reconcile its execution locations with the live environment.
- Do not repeat effects until this reconciliation is complete.

Retrieve long-term memory when needed:
- Retrieve history when observations conflict.
- Retrieve history when an old decision needs explanation.
- Retrieve history before repeating a previously attempted approach.
- Retrieve history when new evidence can invalidate an old conclusion.
- Use include_superseded for retained long-term memory history.
- Follow next_offset until you find relevant history or exhaust the pages.
- Preserve source, time, and verification limits when reconciling versions.
- Task context history is separate from long-term Project Memory.
- Task collaboration history is separate from long-term Project Memory.
- Record a retrieval gap when relevant evidence is unavailable.

Use the Git memory workflow when available:
- Use the Morrows memory CLI when Git and an Agent credential are available.
- Managed runtimes expose the CLI through MORROWS_MEMORY_CLI.
- Use morrows memory checkout for Project memory worktrees.
- Add --history when you need old document bodies.
- Use rg or grep for search.
- Read surrounding context before changing a claim.
- Use native Git for status, diff, log, add, and commit.
- Then use morrows memory publish for one committed document.
- Supply the source Task and ContextRevision you actually read.
- Supply basis, verification status, evidence, and a stable retry key.
- A local Git commit does not publish to Morrows.
- Commit or stash local edits before refresh.
- Refresh preserves merge conflicts for resolution.
- Without the CLI, use paged MCP reads and project_memory_publish.
