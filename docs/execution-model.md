# Execution model

The starter runtime validates target identity and can reject a request with `UnsupportedExecutionMode`. It has no instruction executor. An executor will take a bounded guest thread run request and report its dispatch address, pending guest fault, or exhausted budget. The first backend will evaluate verified IR and serve as the reference for later code generation.

Selecting `Auto` currently returns the same typed error as explicitly requesting an execution backend. Host ISA identity alone cannot establish native execution compatibility.
