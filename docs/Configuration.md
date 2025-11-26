<!--

Copyright (C) 2025 Kian Kasad

This file is part of the Purdue Hackers webring.

The Purdue Hackers webring is free software: you can redistribute it and/or
modify it under the terms of the GNU Affero General Public License as
published by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

The Purdue Hackers webring is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License
for more details.

You should have received a copy of the GNU Affero General Public License along
with the Purdue Hackers webring. If not, see <https://www.gnu.org/licenses/>.
 
-->

# Configuring the webring

The webring is configured via a file [in TOML format][toml].
The rest of this document describes the format of the file and the settings it
can contain.
For the exact implementation, see the `Config` struct in `src/config.rs`.

[toml]: https://toml.io/en/

## TOML introduction

In general, TOML files are made up of tables which contain key-value pairs.
A key can have a table as its value, meaning tables can contain nested tables.
Tables can be written in several ways. All of the following are equivalent in TOML:
- ```toml
  [my-table]
  key = "value"
  ```
- ```toml
  my-table = { key = "value" }
  ```
- ```toml
  my-table.key = "value"
  ```
This flexibility allows you to specify options using whichever form you like.

To provide organization, the webring's settings are split into several tables.
The settings are listed below, grouped into the tables they belong to. Tables
for which no settings are specified can be omitted.

Some settings are required, and some are not. Some have defaults, and some do
not. If a setting is not specified in the configuration file, the following
happens:
| Required | Has default | Result                              |
| ---      | ---         | ---                                 |
| Yes      | No          | Error; webring will not start       |
| Yes      | Yes         | Default value will be used          |
| No       | No          | Setting is empty, i.e. has no value |
| No       | Yes         | Default value will be used          |

## Available settings

### `webring` table

This table contains settings pertaining to the webring server.

| Key          | Required | Type          | Default                          |
| ---          | ---      | ---           | ---                              |
| `base-url`   | no       | string (URL)  | `https://ring.purduehackers.com` |
| `static-dir` | yes      | string (path) | none                             |

Example:
```toml
[webring]
base-url = "https://ring.purduehackers.com"
static-dir = "static"
```

#### `base-url`

Defines the URL of the webring itself. This is used mainly when checking
members' sites to ensure they contain the correct links. The webring must know
its own URL in order to know what the links to it should look like.

This URL must not be relative, i.e. it must have a valid authority (a.k.a.
"host" or "domain"). For example, `file:///ring`, while a valid URL, will be
rejected.

#### `static-dir`

The directory from which static content will be served. Files in this directory
will be served with the path prefix `/static`. E.g. if the value of `static-dir`
is `/www/ring`, and the webring receives a request for `/static/error.html`, it
will serve `/www/ring/error.html`.

This directory also has a special purpose: the `index.html` file is a template
used to render the homepage, populated with the values of the ring's members.
See [`Homepage.md`](./Homepage.md).

Note that this path is relative to the working directory in which the webring is
run, not necessarily relative to the location of the configuration file.
However, it is recommended to run the webring in the directory containing the
configuration file, so that there is no confusion about relative paths.

### `network` table

Settings pertaining to how the webring communicates over the network.

| Key           | Required | Type               | Default |
| ---           | ---      | ---                | ---     |
| `listen-addr` | yes      | string (`ip:port`) | none    |

#### `listen-addr`

Specifies the address on which the webring will listen.

The value must be a string which can be parsed as a [`SocketAddr`][socketaddr],
e.g. an IPv4 or IPv6 address followed by a colon and a port number. For more
details on the format, see [here for IPv4][v4] and [here for IPv6][v6].

[socketaddr]: https://doc.rust-lang.org/std/net/enum.SocketAddr.html
[v4]: https://doc.rust-lang.org/std/net/struct.SocketAddrV4.html#textual-representation
[v6]: https://doc.rust-lang.org/std/net/struct.SocketAddrV6.html#textual-representation

You probably want to use the address `0.0.0.0` or `[::]`, meaning listen on all
available IPv4 or IPv6 interfaces, respectively.

### `logging` table

Controls the webring's logging behavior.

| Key                         | Required | Type                                                             | Default                                             |
| ---                         | ---      | ---                                                              | ---                                                 |
| `verbosity` (alias `level`) | no       | string (one of `off`, `trace`, `debug`, `info`, `warn`, `error`) | `info` for release builds; `debug` for debug builds |
| `log-file`                  | no       | string (path)                                                    | none                                                |

Example:
```toml
[logging]
level = "info"
```

#### `verbosity` (alias `level`)

Sets the minimum severity to log. Messages with this severity or higher will be
logged, and those with lower severity will be suppressed.

For example, at level `info`, messages at `info`, `warn`, and `error` will be
printed, and messages at `trace` and `debug` will not.

The value `off` means no messages will be printed; logging is disabled.

#### `log-file`

Specifies a path to a file to write logs to in addition to the standard error
stream. New log lines will be appended to the file; the webring will never
truncate the file. The file will be created if it does not exist.

### `discord` table

Controls the Discord integration.

| Key           | Required | Type         | Default |
| ---           | ---      | ---          | ---     |
| `webhook-url` | no       | string (URL) | none    |

#### `webhook-url`

Sets the URL of the Discord webhook, which will be executed to send messages.
This must be the full URL of an [Execute Webhook API endpoint][webhook].

If set, members whose sites fail validation (and whose `discord-id` is set) will
be sent a message on Discord explaining the problem. The message is sent to
whichever server and channel the webhook is created for.

[webhook]: https://discord.com/developers/docs/resources/webhook#execute-webhook

### `members` table

This table is a little different; instead of unique settings, it contains the
list of the webring's members. Each key in this table is the name of a member,
and the value is a table configuring the member's site. While all settings
discussed so far apply to the webring as a whole, each member's table is unique
to that member; the settings within do not apply to other members.

Member settings:
| Key           | Required | Type                                                        | Default |
| ---           | ---      | ---                                                         | ---     |
| `url`         | yes      | string (URL)                                                | none    |
| `discord-id`  | no       | integer                                                     | none    |
| `check-level` | no       | string (one of `none`/`off`, `online`/`up`, `links`/`full`) | `full`  |

Examples (all are equivalent):

- ```toml
  [members.kian]
  url = "https://kasad.com"
  discord-id = 123

  [members.henry]
  url = "https://hrovnyak.gitlab.io"
  check-level = "online"
  ```

- ```toml
  [members]
  kian = { url = "https://kasad.com", discord-id = 123 }
  henry = { url = "https://hrovnyak.gitlab.io", check-level = "online" }
  ```

- ```toml
  members.kian = { url = "https://kasad.com", discord-id = 123 }
  members.henry = { url = "https://hrovnyak.gitlab.io", check-level = "online" }
  ```

- ```toml
  members.kian.url = "https://kasad.com"
  members.kian.discord-id = 123
  members.henry.url = "https://hrovnyak.gitlab.io"
  members.henry.check-level = "online"
  ```

#### `url`

The URL of the member's site.

#### `discord-id`

The member's Discord user ID. Note that this is *not* their username, but rather
is a number. See [this article][discord-user-id] for how to find someone's user
ID.

If unset, this member will not be notified on Discord when their site fails checks.

[discord-user-id]: https://support.discord.com/hc/en-us/articles/206346498-Where-can-I-find-my-User-Server-Message-ID

See [the `discord` table](#discord-table) for more on the Discord integration.

#### `check-level`

The level of check to perform on this member's site. Checks are performed on
every site before users are sent there. E.g. if the user requests the next site
in the ring coming from **A**, and site **B** is not online, **B** will be
silently skipped, and the user will be sent to the next passing site, e.g. **C**.

The options are:
- `off` or `none`: No checks are performed. The site is considered to always be
  passing.
- `up` or `online`: The site is considered to pass if it responds to HTTP
  requests with a successful (200-299) response.
- `full` or `links`: The site must pass the previous check, and must also
  respond with an HTML body containing links (`<a>` elements) with the following
  destinations as their destination (`href` attribute):
  - `<RING_BASE_URL>`
  - `<RING_BASE_URL>/prev` or `<RING_BASE_URL>/previous` (with optional `?host=` parameter)
  - `<RING_BASE_URL>/next` (with optional `?host=` parameter)

  If any of these links is missing, the site fails the check. Additionally,
  elements of any kind with a `data-phwebring` attribute containing the value
  `home`, `prev`/`previous`, or `next` will be accepted instead of a link
  element.

## Live reloading

If the configuration file is changed while the webring is running, this change
will be detected, and the webring will attempt to re-load the member list.
Only the `members` table is re-loaded; other changes will not be applied.

If the updated configuration file is invalid, an error will be logged and the
update will be ignored, leaving the webring running the old configuration.
