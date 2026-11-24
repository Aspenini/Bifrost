# Security

Report suspected vulnerabilities privately to the project maintainer. Include the affected version, the input structure, and steps needed to reproduce the issue. Omit sensitive host or guest data from the report. A public advisory or project-specific reporting address will be documented once the repository has a configured private reporting channel.

Unsafe Rust is scoped to executable memory publication, generated-code entry, checked memory and ABI callbacks, and native reference fixtures. Both execution backends check guest spans, alignment, permissions and instruction budgets. The JIT publishes immutable executable pages and does not execute guest buffers directly. The ABI bridge forwards only explicit typed bindings from trusted native libraries. Hosting guest code in the same process does not establish a security sandbox.

CI checks RustSec advisories and dependency licenses using `cargo deny --locked --all-features check advisories licenses`. The workspace tests also apply deterministic mutations and truncations to the self-authored ELF, PE, and Mach-O fixtures and check that inspection handles them without panicking.
