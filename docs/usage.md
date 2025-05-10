## test

```shell
# Linux
cargo build --manifest-path=example/read_link/Cargo.toml
LD_PRELOAD=example/read_link/target/debug/libreadlinkspy.so ls -l .

# macos
cargo build --manifest-path=example/read_link/Cargo.toml
DYLD_INSERT_LIBRARIES=example/read_link/target/debug/libreadlinkspy.dylib DYLD_FORCE_FLAT_NAMESPACE=1 ls -l .
```