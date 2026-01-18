/*
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
*/

function setFlip(flip) {
    document.body.style.transform = flip ? "scaleX(-1)" : "";
    const links = document.getElementsByTagName("a");
    for (const link of links) {
        if (flip) {
            link.href = "/flip?url=" + encodeURIComponent(link.href);
        } else {
            const url = new URL(link.href);
            if (url.pathname === "/flip") {
                link.href = url.searchParams.get("url");
            }
        }
    }
}

document.getElementsByTagName("a").forEach(a => {
    if (a.host !== window.location.host && !a.getAttribute("data-umami-event")) {
        a.setAttribute("data-umami-event", "outbound-link-click");
        a.setAttribute("data-umami-event-url", a.href);
    }
});
