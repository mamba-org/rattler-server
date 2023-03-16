# rattler-server: resolve conda envs on the fly

`rattler-server` is a single-purpose tool to resolve conda environments with a HTTP endpoint.
The tool uses crates from the lower-level [`rattler`](https://github.com/mamba-org/rattler) libraries.

Features:

* Written in Rust, memory safe and fast!
* Uses axum for async, parallel request execution
* Fast caching with libsolv and configurable cache lifetime
* Uses the same package resolve algorithms as `mamba`

### The CLI interface

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
  "name": "<any name>",
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

The server will reply with the solved, topologically sorted dependencies for that environment, e.g.:

```json
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