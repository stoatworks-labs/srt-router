# Attributions

SRT Router is built on other people's work. This file lists what that work is, who did
it, and what it is doing here.

It is generated — the master lists live in the `stoatworks-backend` repo and are
pushed out by `scripts/sync-attributions.py`. Edit it there, not here.

## Third-party code this project uses

Libraries, SDKs and frameworks the project is built on or bundles.

### NDI SDK

<https://ndi.video/for-developers/ndi-sdk/>  
Licence: NDI SDK Licence Agreement (proprietary)  
Copyright: Vizrt Group

Headers only, vendored so the backend compiles everywhere. The runtime is never redistributed — it is loaded with dlopen at run time if the user has installed it.

NDI is the video transport most of this fleet's users already run. Compiling against the headers without shipping the runtime keeps the licence intact and still gives every build the backend.

### Open Media Transport (libomt)

<https://github.com/openmediatransport/libomtnet>  
Licence: MIT  
Copyright: Open Media Transport Contributors

Header vendored alongside the NDI headers; the library is loaded at run time.

The open alternative to NDI, and MIT end to end — so unlike NDI it can be supported without a proprietary licence in the path.

### The Rust crate ecosystem

<https://crates.io>  
Licence: predominantly MIT or Apache-2.0  
Copyright: the individual crate authors

Cargo dependencies, resolved and pinned in Cargo.lock.

Async runtimes, protocol codecs, serialisation and GUI toolkits. The exact set and versions for any build are in that repo's Cargo.lock, which is the authoritative list.

The full transitive dependency set for any build is pinned in this repo's lockfile,
which is the authoritative list. What is named above is the layers a reader would
want to know about, not every package that has ever been resolved.

## Getting this wrong

If your work is here and the description is inaccurate, the licence is wrong, or you would rather not be listed — open an issue and it will be fixed.
