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
            params.set("horizontal", "true");
        }
        if (isAusFlipEnabled) {
            params.set("vertical", "true");
        }
        link.href = "/flip?" + params.toString();
    }
}

// Kept as global functions for compatibility with existing bookmarks/scripts.
function setFlip(flip) {
    isFlipEnabled = flip;
    updateFlipState();
}

function setAusFlip(flip) {
    isAusFlipEnabled = flip;
    updateFlipState();
}

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

        row.addEventListener("pointermove", event => {
            if (event.pointerType && event.pointerType !== "mouse") {
                return;
            }
            preview.style.setProperty("--preview-x", `${event.clientX}px`);
            preview.style.setProperty("--preview-y", `${event.clientY}px`);
        });
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

    buttons.forEach(button => {
        button.addEventListener("click", () => setView(button.dataset.view));
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
    let pointerStartX = null;
    let pointerCurrentX = null;
    let isDragging = false;
    let currentTranslate = 0;
    let prevTranslate = 0;
    let animationFrameId = null;
    let suppressClick = false;
    let wheelTimeout; // debounce timer for wheel snapping

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

    prevBtn?.addEventListener("click", showPrevious);
    nextBtn?.addEventListener("click", showNext);

    const carouselTrack = carousel.querySelector(".carousel-track");

    function setSliderPosition() {
        carouselTrack.style.transform = `translateX(${currentTranslate}px)`;
    }

    function animation() {
        setSliderPosition();
        if (isDragging) {
            animationFrameId = requestAnimationFrame(animation);
        }
    }

    function clamp(value, min, max) {
        return Math.min(Math.max(value, min), max);
    }

    carousel?.addEventListener("pointerdown", event => {
        pointerStartX = event.clientX;
        pointerCurrentX = pointerStartX;
        isDragging = true;
        carousel.classList.add("is-dragging");
        carousel.setPointerCapture?.(event.pointerId);

        // Calculate currentTranslate based on current active slide
        currentTranslate = -current * carousel.clientWidth;
        prevTranslate = currentTranslate;
        animationFrameId = requestAnimationFrame(animation);
    });

    carousel?.addEventListener("pointermove", event => {
        if (!isDragging) return;
        pointerCurrentX = event.clientX;
        const deltaX = pointerCurrentX - pointerStartX;
        currentTranslate = clamp(
            prevTranslate + deltaX,
            -((slides.length - 1) * carousel.clientWidth),
            0,
        );
    });

    carousel?.addEventListener("pointerup", event => {
        if (!isDragging) return;
        isDragging = false;
        cancelAnimationFrame(animationFrameId);

        const deltaX = event.clientX - pointerStartX;
        carousel.classList.remove("is-dragging");

        // Snap to closest slide index
        const movedSlides = Math.round(-currentTranslate / carousel.clientWidth);
        current = clamp(movedSlides, 0, slides.length - 1);

        // Reset translate to snapped slide
        currentTranslate = -current * carousel.clientWidth;
        setSliderPosition();

        suppressClick = Math.abs(deltaX) > 10;

        render();
    });

    carousel?.addEventListener("pointercancel", () => {
        if (!isDragging) return;
        isDragging = false;
        cancelAnimationFrame(animationFrameId);
        currentTranslate = -current * carousel.clientWidth;
        setSliderPosition();
        carousel.classList.remove("is-dragging");
    });

    carousel?.addEventListener(
        "wheel",
        event => {
            const horizontalDistance =
                Math.abs(event.deltaX) > Math.abs(event.deltaY)
                    ? event.deltaX
                    : event.shiftKey
                      ? event.deltaY
                      : 0;

            if (!horizontalDistance) {
                return;
            }

            event.preventDefault();

            // Adjust currentTranslate immediately based on wheel delta
            currentTranslate = clamp(
                currentTranslate - horizontalDistance,
                -((slides.length - 1) * carousel.clientWidth),
                0,
            );
            setSliderPosition();

            // Debounce snapping to slide when wheel scroll stops
            clearTimeout(wheelTimeout);
            wheelTimeout = setTimeout(() => {
                const movedSlides = Math.round(-currentTranslate / carousel.clientWidth);
                current = clamp(movedSlides, 0, slides.length - 1);
                currentTranslate = -current * carousel.clientWidth;
                setSliderPosition();
                render();
            }, 100);
        },
        { passive: false },
    );

    carousel?.addEventListener(
        "click",
        event => {
            if (suppressClick) {
                event.preventDefault();
                suppressClick = false;
            }
        },
        true,
    );

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
