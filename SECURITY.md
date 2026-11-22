# Security

Report suspected vulnerabilities privately to the project maintainer. Include the affected version, the input structure, and steps needed to reproduce the issue. Omit sensitive host or guest data from the report. A public advisory or project-specific reporting address will be documented once the repository has a configured private reporting channel.

The starter code uses Rust with unsafe code forbidden. The address space uses owned guest bytes and checks the full span before copying. IR verification establishes structure and types. A future backend must enforce guest address, alignment, permission, resource, and fault rules at runtime. Hosting guest code in the same process does not establish a security sandbox.
