# Security

Report suspected vulnerabilities privately to the project maintainer. Include the affected version, the input structure, and steps needed to reproduce the issue. Omit sensitive host or guest data from the report. A public advisory or project-specific reporting address will be documented once the repository has a configured private reporting channel.

Runtime and library crates forbid unsafe Rust. The differential test crate permits inline assembly only in its fixed native reference module. The address space uses owned guest bytes and checks the full span before copying. IR verification establishes structure and types. The interpreter enforces guest address, alignment, permissions, and instruction budgets at runtime. Hosting guest code in the same process does not establish a security sandbox.
