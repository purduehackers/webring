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

let isFlipEnabled = false;
let isAusFlipEnabled = false;

function updateFlipState() {
    const transforms = [];
    if (isFlipEnabled) {
        transforms.push("scaleX(-1)");
    }
    if (isAusFlipEnabled) {
        transforms.push("scaleY(-1)");
    }
    document.body.style.transform = transforms.join(" ");

    const links = document.getElementsByTagName("a");
    for (const link of links) {
        const originalHref = link.dataset.originalHref || link.href;
        link.dataset.originalHref = originalHref;

        if (!isFlipEnabled && !isAusFlipEnabled) {
            link.href = originalHref;
            continue;
        }

        const params = new URLSearchParams({ url: originalHref });
        if (isFlipEnabled) {
            params.set("vertical", "true");
        }
        if (isAusFlipEnabled) {
            params.set("horizontal", "true");
        }
        link.href = "/flip?" + params.toString();
    }
}

function setFlip(flip) {
    isFlipEnabled = flip;
    updateFlipState();
}

function setAusFlip(flip) {
    isAusFlipEnabled = flip;
    updateFlipState();
}

document.getElementsByTagName("a").forEach(a => {
    if (a.host !== window.location.host && !a.getAttribute("data-umami-event")) {
        a.setAttribute("data-umami-event", "outbound-link-click");
        a.setAttribute("data-umami-event-url", a.href);
    }
});
