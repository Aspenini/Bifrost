# Debugging the starter

Run the verification example to view the fixture's guest bytes and a human-readable listing tagged with guest PCs. IR errors expose a block ID, operation PC if applicable, and error kind. Memory errors report the first failing byte address and access kind for a permission violation.

If a component reports unavailable after enabling a feature, check the [capability matrix](capability-matrix.md). The starter feature flags select reserved crate boundaries. No instruction trace is produced until an executor is available.
