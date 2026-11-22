# Security

Report suspected vulnerabilities privately to the project maintainer. Include the affected version, the input structure, and steps needed to reproduce the issue. Omit sensitive host or guest data from the report. A public advisory or project-specific reporting address will be documented once the repository has a configured private reporting channel.

Unsafe Rust is scoped to executable memory publication, the generated-code entry and checked memory callback, and fixed native test assembly. Both execution backends check guest spans, alignment, permissions and instruction budgets. The JIT publishes immutable executable pages and does not execute guest buffers directly. Hosting guest code in the same process does not establish a security sandbox.
