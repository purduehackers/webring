<!--
Copyright Amolith <amolith@secluded.site>
Copyright (C) 2024 Kian Kasad <kian@kasad.com>

SPDX-License-Identifier: CC0-1.0
-->

# go-webring

Fork of [go-webring][upstream] customized for the [Purdue
Hackers](https://purduehackers.com) [webring](https://ring.purduehackers.com).

Fork-specific features:
- Notify site owners via Discord when their webring links are broken/missing.

## Usage

``` text
$ ./go-webring -h
Usage of ./go-webring
  -c, --contact string         Contact instructions for errors (default "contact the admin and let them know what's up")
  -h, --host string            Host this webring runs on, primarily used for validation
  -i, --index string           Path to home page template (default "index.html")
  -l, --listen string          Host and port go-webring will listen on (default "127.0.0.1:2857")
  -m, --members string         Path to list of webring members (default "list.txt")
  -v, --validationlog string   Path to validation log, see docs for requirements (default "validation.log")
```

This webring implementation handles four paths:
- **Root:** returns the home page template replacing the string "`{{ . }}`" with
  an HTML table of ring members
- **Next:** returns a 302 redirect pointing to the next site in the list
- **Previous:** returns a 302 redirect pointing to the previous site in the list
- **Random:** returns a 302 redirect pointing to a random site in the list
- **\$validationlog:** displays the [validation log](#validation) at the path
  specified in the command line flags
  - For example, with `-v validationlog -h example.com`, the path would be
    `example.com/validationlog`

The **next** and **previous** paths require a `?host=` parameter containing a
URL-encoded URI of the site being visited. For example, if Sam is a member of a
webring on `example.com` and her site is `sometilde.com/~sam`, she will need the
following links on her page for directing visitors to the next/previous ring
members.

- `https://example.com/next?host=sometilde.com%2F~sam`
- `https://example.com/previous?host=sometilde.com%2F~sam`

### With provided examples

See the included `list.txt` and `index.md` for examples of a webring setup. To
run `go-webring` with those examples, first install [pandoc](https://pandoc.org)
then generate `index.html` from `index.md` like so:

``` shell
$ pandoc -s index.md -o index.html
```

Next, you'll need to [install Go](https://go.dev/dl) and build the project.

``` shell
$ go build
```

After that, simply execute the binary then open
[localhost:2857](http://localhost:2857) in your browser.

``` shell
$ ./go-webring
```

### With custom files

To run your own webring, you'll first need a template homepage. This should be
any HTML file with the string "`{{ . }}`" placed wherever you want the table of
members inserted. This table is plain HTML so you can style it with CSS.

Pandoc produces very pleasing (in my opinion) standalone HTML pages; if you just
want something simple, I would recommend modifying the included `index.md` and
generating your homepage as in section above.

To serve other assets, such as styles in a separate `.css` file, images, etc.,
place them in the `static/` directory; a file at `static/favicon.ico` will be
accessible at `https://example.com/static/favicon.ico`.

Next, you'll need a text file containing a list of members. On each line should
be the member's unique identifer (such as their username), their Discord user
ID, and their site's URI omitting the scheme, each separated by a space. For
example, if a user is `bob` and his site is `https://bobssite.com`, his line
would look like the following.

``` text
bob 123456789123456789 bobssite.com
```

See the [Discord notifications](#discord-notifications) section for details on
the format of the Discord user ID column.

If the user was `sam` and her site was `https://sometilde.com/~sam`, her line
would look like this:

``` text
sam - sometilde.com/~sam
```

With those two members in the text file, the HTML inserted into the home page
will be the following.

``` html
<tr>
  <td>bob</td>
  <td><a href="https://bobssite.com">bobssite.com</a><td>
</tr>
<tr>
  <td>sam</td>
  <td><a href="https://sometilde.com/~sam">sometilde.com/~sam</a><td>
</tr>
```

Assuming this webring is on `example.com`, Bob will need to have the following
links on his page.

- `https://example.com/next?host=bobssite.com`
- `https://example.com/previous?host=bobssite.com`

Because Sam has a forward slash in her URI, she'll need to percent-encode it so
browsers interpret the parameter correctly.

- `https://example.com/next?host=sometilde.com%2F~sam`
- `https://example.com/previous?host=sometilde.com%2F~sam`


## Validation

At startup, a concurrent process spins off, checks every member's site for
issues, generates a report, and serves the report at the location specified in
the command line flag. It rechecks sites every 24 hours and identifies TLS
errors, unreachable sites, and sites with missing links. It will eventually
follow redirects too, allowing members to move their site without having to
notify ring maintainers.

There are some false positives right now, but I'm working on correcting those.

### Discord notifications

When a problem is found with a site, a ping can be sent to the Discord user
associated with the site. To enable this, enter the Discord user ID to ping as
the second column of an entry in the member list file.

Note that the user ID is different from the username.
[See here for how to find your user ID.](https://support.discord.com/hc/en-us/articles/206346498-Where-can-I-find-my-User-Server-Message-ID)

To disable notifications for a user, enter a single hyphen as their user ID.
The example of the user `sam` above shows this.

Finally, you'll need to create a Discord webhook for the server/channel in which
you want to send the pings. Once this is done, place the webhook URL in a file
and specify the path to this file using the command-line options. You can also
[specify a specific thread using query parameters in the URL][thread-param].

[thread-param]: https://discord.com/developers/docs/resources/webhook#execute-webhook-query-string-params

## Questions & Contributions
For the upstream go-webring project, [see here][upstream].

If you have questions/comments/suggestions about this project, feel free to open
an issue or pull request at <https://github.com/kdkasad/go-webring>.

[upstream]: https://git.sr.ht/~amolith/go-webring

