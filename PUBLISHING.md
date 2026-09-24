# Publishing `colint` to PyPI

The PyPI distribution name is `colint`, owned by the `ColinKennedy` PyPI account. It packages the Rust `colint` executable with Maturin's `bin` bindings, so users install and run it with:

```shell
python -m pip install colint
colint --help
```

## One-time setup

1. Create the `colint` project under the `ColinKennedy` PyPI account (or use a first publish from the release workflow).
2. Push this repository to GitHub.
3. In PyPI project settings, add a trusted publisher for that GitHub repository and the `publish-pypi.yml` workflow. The workflow uses OpenID Connect; no PyPI token is stored in GitHub.

## Release

1. Update the matching versions in `Cargo.toml` and `pyproject.toml`.
2. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all-targets`.
3. Create and publish a GitHub release tagged `vX.Y.Z`.

The release workflow builds platform wheels and an sdist, then publishes the assembled artifacts through PyPI trusted publishing.
