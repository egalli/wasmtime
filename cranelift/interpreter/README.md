This crate provides an interpreter for Cranelift IR. It is still a work in progress, as many
instructions are unimplemented and various implementation gaps exist. Use at your own risk.

### Test

To test the cranelift interpreter:

```shell script
cargo test --package cranelift-interpreter --lib
```

(The `--lib` helps the debugger identify the correct binary to debug.)
