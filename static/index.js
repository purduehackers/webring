/*
Copyright (C) 2025 Kian Kasad and Amber Zeng

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

function initOutboundLinkTracking() {
    document.querySelectorAll("a").forEach(link => {
        if (link.host !== window.location.host && !link.dataset.umamiEvent) {
            link.setAttribute("data-umami-event", "outbound-link-click");
            link.setAttribute("data-umami-event-url", link.href);
        }
    });
}

function initListPreviewCursor() {
    document.querySelectorAll(".member-list-row").forEach(row => {
        const preview = row.querySelector(".member-list-preview");
        if (!preview) {
            return;
        }

        function movePreview(event) {
            if (event.pointerType && event.pointerType !== "mouse") {
                return;
            }
            preview.style.position = "fixed";
            preview.style.transform = "none";
            preview.style.left = `${event.clientX - preview.offsetWidth - 12}px`;
            preview.style.top = `${event.clientY - preview.offsetHeight - 12}px`;
        }

        row.addEventListener("pointerenter", movePreview);
        row.addEventListener("pointermove", movePreview);
        row.addEventListener("mousemove", movePreview);
    });
}

function initViewToggle() {
    const buttons = Array.from(document.querySelectorAll(".view-toggle-button"));
    const panels = Array.from(
        document.querySelectorAll("[data-view-panel], #carousel-view, #list-view"),
    );
    if (!buttons.length || !panels.length) {
        return;
    }

    let savedView = "carousel";
    try {
        savedView = localStorage.getItem("webring:view") || savedView;
    } catch {
        // Local storage may be unavailable in private browsing contexts.
    }

    let transitionTimer;
    const VIEW_SWITCH_DURATION = 230;

    function setView(view, animate = true) {
        const nextView = view === "list" ? "list" : "carousel";
        const nextPanel = document.getElementById(`${nextView}-view`);
        const currentPanel = panels.find(panel => !panel.hidden);
        if (!nextPanel) {
            return;
        }

        buttons.forEach(button => {
            button.setAttribute("aria-pressed", String(button.dataset.view === nextView));
        });
        try {
            localStorage.setItem("webring:view", nextView);
        } catch {
            // The view still works for the current session.
        }

        window.clearTimeout(transitionTimer);
        document.body.classList.remove("view-switching");
        panels.forEach(panel => panel.classList.remove("is-entering", "is-leaving"));

        if (!animate || !currentPanel || currentPanel === nextPanel) {
            panels.forEach(panel => {
                panel.hidden = panel !== nextPanel;
            });
        } else {
            currentPanel.classList.add("is-leaving");
            nextPanel.hidden = false;
            nextPanel.classList.add("is-entering");
            document.body.classList.add("view-switching");

            transitionTimer = window.setTimeout(() => {
                currentPanel.hidden = true;
                currentPanel.classList.remove("is-leaving");
                nextPanel.classList.remove("is-entering");
                document.body.classList.remove("view-switching");
            }, VIEW_SWITCH_DURATION);
        }
    }

    const toggleSound = new Audio("/static/click2.mp3");
    toggleSound.preload = "auto";

    function playToggleSound() {
        toggleSound.currentTime = 0;
        toggleSound.play().catch(() => {});
    }

    buttons.forEach(button => {
        button.addEventListener("click", () => {
            playToggleSound();
            setView(button.dataset.view);
        });
    });
    setView(savedView, false);
}

function initCarousel() {
    const slides = Array.from(document.querySelectorAll(".carousel-slide"));
    if (!slides.length) {
        return;
    }

    const nameLabel = document.getElementById("current-name");
    const prevBtn = document.getElementById("prev-btn");
    const nextBtn = document.getElementById("next-btn");
    const carousel = document.querySelector(".carousel");
    let current = 0;

    function memberName(slide) {
        return slide.querySelector(".preview-frame")?.dataset.umamiEventName || "";
    }

    function render() {
        slides.forEach((slide, index) => {
            slide.classList.remove("is-current", "is-prev", "is-next");
            if (index === current) {
                slide.classList.add("is-current");
            } else if (index === (current - 1 + slides.length) % slides.length) {
                slide.classList.add("is-prev");
            } else if (index === (current + 1) % slides.length) {
                slide.classList.add("is-next");
            }
        });

        if (nameLabel) {
            nameLabel.textContent = memberName(slides[current]);
        }
    }

    function showPrevious() {
        current = (current - 1 + slides.length) % slides.length;
        render();
    }

    function showNext() {
        current = (current + 1) % slides.length;
        render();
    }

    const clickSound = new Audio("/static/click.mp3");
    clickSound.preload = "auto";
    clickSound.volume = 0.5;

    function playClick() {
        clickSound.currentTime = 0;
        clickSound.play().catch(() => {});
    }

    prevBtn?.addEventListener("click", () => {
        playClick();
        showPrevious();
    });
    nextBtn?.addEventListener("click", () => {
        playClick();
        showNext();
    });

    document.addEventListener("keydown", event => {
        if (!document.getElementById("list-view")?.hidden) {
            return;
        }
        if (event.key === "ArrowLeft") {
            showPrevious();
        } else if (event.key === "ArrowRight") {
            showNext();
        }
    });

    render();
}

document.addEventListener("DOMContentLoaded", () => {
    initOutboundLinkTracking();
    initListPreviewCursor();
    initViewToggle();
    initCarousel();
});
