# Third-party notices

`@agentgit/client` has **no runtime dependencies**. Its Ed25519 and SHA-256 implementations come from
two small, dependency-free libraries by Paul Miller, which are bundled into `dist/` (they are
published as ES modules only, and bundling keeps the CommonJS build working on every Node 20).
Both are MIT-licensed:

| Package                                                        | Version | Used for                                          | License |
| -------------------------------------------------------------- | ------- | ------------------------------------------------- | ------- |
| [`@noble/ed25519`](https://github.com/paulmillr/noble-ed25519) | 3.2.0   | signing and verifying provenance and attestations | MIT     |
| [`@noble/hashes`](https://github.com/paulmillr/noble-hashes)   | 2.4.0   | SHA-256 (content ids) and SHA-512 (Ed25519)       | MIT     |

```text
The MIT License (MIT)

Copyright (c) 2019 Paul Miller (https://paulmillr.com)   [@noble/ed25519]
Copyright (c) 2022 Paul Miller (https://paulmillr.com)   [@noble/hashes]

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```
