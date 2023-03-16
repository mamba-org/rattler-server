# rattler-server: resolve conda envs on the fly

`rattler-server` is a single-purpose tool to resolve conda environments with a HTTP endpoint.
The tool uses crates from the lower-level [`rattler`](https://github.com/mamba-org/rattler) libraries.

If you want to learn more, join our [Discord!](https://discord.gg/c5gVKJpKGa).

Features:

* Written in Rust, memory safe and fast!
* Uses axum for async, parallel request execution
* Fast caching with libsolv and configurable cache lifetime
* Uses the same package resolve algorithms as [`mamba`](https://github.com/mamba-org/mamba)


### The CLI interface

If you clone this repository, you can run `rattler-server` by using

```
# run rattler-server on default port 3000
cargo run

# or to run on another port (3322)
cargo run -- -p 3322
```

The full help text is as follows:

```
Usage: rattler-server [OPTIONS]

Options:
  -p <PORT>
          The port at which the server should listen [env: RATTLER_SERVER_PORT=] [default: 3000]
  -c <CONCURRENT_REPODATA_DOWNLOADS_PER_REQUEST>
          The amount of concurrent downloads of repodata.json files, during a single request. JSON downloads are very CPU-intensive, because they require parsing huge JSON bodies [env: RATTLER_SERVER_PORT_CONCURRENT_DOWNLOADS=] [default: 1]
  -r <REPODATA_CACHE_EXPIRATION_SECONDS>
          The amount of seconds after which a cached repodata.json expires, defaults to 30 minutes [env: RATTLER_SERVER_CACHE_EXPIRATION_SECONDS=] [default: 1800]
  -h, --help
          Print help
```

### The endpoints

It has a single endpoint (`/solve`) that accepts HTTP POST requests with the following JSON content:

```json
{
  "specs": [
    "cudnn",
    "tensorflow-gpu"
  ],
  "virtual_packages": ["__glibc=2.5=0", "__cuda=11=0"],
  "channels": [
    "conda-forge"
  ],
  "platform": "linux-64"
}
```

If successful, the server will reply a HTTP 200 Response with the solved, topologically sorted dependencies for that environment as JSON, e.g.:

```json5
{
  "packages": [
    {
      "name": "_libgcc_mutex",
      "version": "0.1",
      "build": "conda_forge",
      "build_number": 0,
      "subdir": "linux-64",
      "md5": "d7c89558ba9fa0495403155b64376d81",
      "sha256": "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
      "size": 2562,
      "depends": [],
      "constrains": [],
      "license": "None",
      "timestamp": 1578324546067,
      "fn": "_libgcc_mutex-0.1-conda_forge.tar.bz2",
      "url": "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
      "channel": "https://conda.anaconda.org/conda-forge/"
    },
    {
      "name": "libgomp",
      "version": "12.2.0",
      "build": "h65d4601_19",
      "build_number": 19,
      "subdir": "linux-64",
      "md5": "cedcee7c064c01c403f962c9e8d3c373",
      "sha256": "81a76d20cfdee9fe0728b93ef057ba93494fd1450d42bc3717af4e468235661e",
      "size": 466188,
      "depends": [
        "_libgcc_mutex 0.1 conda_forge"
      ],
      "constrains": [],
      "license": "GPL-3.0-only WITH GCC-exception-3.1",
      "license_family": "GPL",
      "timestamp": 1666519598453,
      "fn": "libgomp-12.2.0-h65d4601_19.tar.bz2",
      "url": "https://conda.anaconda.org/conda-forge/linux-64/libgomp-12.2.0-h65d4601_19.tar.bz2",
      "channel": "https://conda.anaconda.org/conda-forge/"
    },
    // ... and many more
  ]
}
```

If you ask for an unsolvable environment (e.g. by using an old `__glibc=1.0=0` virtual package), a HTTP 409 response with the following content is returned:

```json
{
  "error_kind": "solver",
  "message": "no solution found for the specified dependencies",
  "additional_info": [
    "nothing provides __glibc >=2.17,<3.0.a0 needed by cudnn-8.2.0.53-h86fa8c9_0"
  ]
}
```