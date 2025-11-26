# Purdue Hackers webring

[![GitHub Actions Workflow Status badge](https://img.shields.io/github/actions/workflow/status/purduehackers/webring/ci.yml?logo=github&label=CI)](https://github.com/purduehackers/webring/actions/workflows/ci.yml)
[![Codecov badge](https://img.shields.io/codecov/c/github/purduehackers/webring?logo=codecov&color=%23F01F7A)](https://app.codecov.io/gh/purduehackers/webring)
[![License badge](https://img.shields.io/github/license/purduehackers/webring?color=blue)](COPYING)

A [webring] for the members and friends of the [Purdue Hackers] club.

This project is a Rust web server which provides the functionality for our
webring. It allows members to link from their sites to the webring, and upon a
user clicking such a link, the webring server will figure out the next member in
the ring and redirect the user there. It has some extra niceties which are
described below.

[webring]: https://en.wikipedia.org/wiki/Webring
[Purdue Hackers]: https://purduehackers.com

# Features
- [Fast & lightweight](#design)
- [Easy configuration] using a TOML file
- Customizable index & error pages using templates
- Beautiful error handling (even for crashes — [try it!][panic])
- Automatically skips sites that are down or broken
- Notifies members on Discord when an issue is detected with their site
- High test coverage to ensure program correctness

[Easy configuration]: ./docs/Configuration.md
[panic]: https://ring.purduehackers.com/debug/panic

# Design
The server is written in [Rust] using the [Tokio] asynchronous runtime. This
allows single threads to process possibly many requests at once, giving us
incredible performance. We make heavy use of caching and lock-free atomics where
possible to ensure each request is processed as quickly as it can be.

In a not-very-academically-rigorous benchmark, we were able to serve ~15,000 requests per second from one instance of the webring running on an [AWS EC2 t2.micro] VM.

[Rust]: https://rust-lang.org
[Tokio]: https://tokio.rs
[AWS EC2 t2.micro]: https://aws.amazon.com/ec2/instance-types/t2/

# License
Copyright (C) 2025 members of Purdue Hackers

The Purdue Hackers webring is free software: you can redistribute it and/or
modify it under the terms of the [GNU Affero General Public License](./COPYING)
as published by the Free Software Foundation, either version 3 of the License,
or (at your option) any later version.

# Want to join?
0. Be a member of Purdue Hackers.

1. Add the following links to your site:
   ```html
   <a href="https://ring.purduehackers.com/previous">Previous</a>
   <a href="https://ring.purduehackers.com/">Purdue Hackers webring</a>
   <a href="https://ring.purduehackers.com/next">Next</a>
   ```
   You can style them how you'd like, e.g. by replacing the text of each link with whatever you want. The only real requirement we have is that they must be visible on your site's homepage, not hidden away somewhere.

2. Click the next or previous link on your site. It should return a "400 Bad Request" error with the reason being that you're not a member of the webring.

   If it instead says that the request doesn't indicate which site it comes from, try the following:
   1. Add the `referrerpolicy="origin"` attribute to the links in your HTML.
   2. Add `?host=<your-domain>` to the `/next` and `/previous` URLs.
   3. Ping Kian in [#webring] after trying the two steps above.

3. Send a message in [#webring] with your desired name (for the index page) and your site URL.
   
[#webring]: https://discord.com/channels/772576325897945119/1319140464812753009
