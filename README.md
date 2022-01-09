# gRPC Leader Election

This is my implementation of the half-duplex alternating variant of the eager
[leader election algorithm on a ring
topology](https://courses.fit.cvut.cz/NI-DSV/lectures/NI-DSV-Prednaska04-LeaderElection.pdf#Outline0.3).

## Lessons Learned

- gRPC is a horrible framework for peer-to-peer communication, at least with the
  client/server abstraction (I'm not sure if that's universal across gRPC
  libraries)
  - modelling a node with the client/server model requires synchronisation on
    shared state, which is painful to manage
  - bidirectional streaming requires a lot of boilerplate (not in terms of LOC,
    in terms of concepts that have to come together to enable it) and there's no
    straightforward API for it -- you have to mix features from `tonic`,
    `tokio`, `tokio-stream`, `async-stream`, and `futures`. Some of these crates
    have similar names for different things which can't be mixed up
  - the complexity of the types involved leads to hard to read compilation
    errors, sometimes because type or (particularly) lifetime inference fails
  - generated code doesn't support ergonomic IDE features such as go to
    definition and the complications of the compilation process slow
    `rust-analyzer` down considerably -- it is often faster to read errors from
    the command line by manually invoking `cargo` than to wait for
    `rust-analyzer` to update the in-editor error highlights. Other features
    like documentation on hover or symbol renaming are slow and sometimes
    near-unusable too
