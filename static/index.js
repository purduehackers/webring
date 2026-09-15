/*
Copyright (C) 2025 members of Purdue Hackers

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

function initLogoAnimation() {
    const logo = document.querySelector(".logo");
    const image = logo?.querySelector("img");
    if (!logo || !image || window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
        return;
    }

    const originalFrame = image.src;
    const frames = [1, 2, 3, 4].map(
        frame => `/static/frames/frame${frame}.svg`,
    );
    frames.forEach(src => {
        const preload = new Image();
        preload.src = src;
    });

    let timer;
    let frame = 0;
    let hovered = false;
    let focused = false;

    function stop() {
        window.clearInterval(timer);
        timer = undefined;
        frame = 0;
        image.src = originalFrame;
    }

    function start() {
        if (timer) {
            return;
        }
        image.src = frames[0];
        timer = window.setInterval(() => {
            frame = (frame + 1) % frames.length;
            image.src = frames[frame];
        }, 120);
    }

    function sync() {
        if (hovered || focused) {
            start();
        } else {
            stop();
        }
    }

    logo.addEventListener("mouseenter", () => {
        hovered = true;
        sync();
    });
    logo.addEventListener("mouseleave", () => {
        hovered = false;
        sync();
    });
    logo.addEventListener("focusin", () => {
        focused = true;
        sync();
    });
    logo.addEventListener("focusout", () => {
        focused = false;
        sync();
    });
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

        document.body.appendChild(preview);

        let previewSize;
        let moveFrame;
        let pendingPosition;

        function positionPreview(clientX, clientY) {
            if (!previewSize) {
                const bounds = preview.getBoundingClientRect();
                previewSize = { width: bounds.width, height: bounds.height };
            }

            const gap = 12;
            const { width, height } = previewSize;
            let left = clientX + gap;
            let top = clientY + gap;

            if (left + width > window.innerWidth - gap) {
                left = clientX - width - gap;
            }
            if (top + height > window.innerHeight - gap) {
                top = clientY - height - gap;
            }

            preview.style.left = `${Math.max(gap, left)}px`;
            preview.style.top = `${Math.max(gap, top)}px`;
            preview.classList.add("is-visible");
        }

        function movePreview(event) {
            if (event.pointerType && event.pointerType !== "mouse") {
                return;
            }
            pendingPosition = [event.clientX, event.clientY];
            if (moveFrame) {
                return;
            }
            moveFrame = window.requestAnimationFrame(() => {
                moveFrame = undefined;
                positionPreview(...pendingPosition);
            });
        }

        function showFocusedPreview() {
            const bounds = row.getBoundingClientRect();
            // Anchor preview to row top right
            positionPreview(bounds.right, bounds.top);
        }

        function hidePreview(event) {
            if (event.pointerType && event.pointerType !== "mouse") {
                return;
            }
            if (moveFrame) {
                window.cancelAnimationFrame(moveFrame);
                moveFrame = undefined;
            }
            preview.classList.remove("is-visible");
        }

        row.addEventListener("pointerenter", movePreview);
        row.addEventListener("pointermove", movePreview);
        row.addEventListener("pointerleave", hidePreview);
        row.addEventListener("focusin", showFocusedPreview);
        row.addEventListener("focusout", event => {
            if (!row.contains(event.relatedTarget)) {
                preview.classList.remove("is-visible");
            }
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
    const randomScream = new Audio("/static/scream.mp3");
    let current = Math.floor(Math.random() * slides.length);

    function memberName(slide) {
        return slide.querySelector(".preview-frame")?.dataset.umamiEventName || "";
    }

    let exitTimer;
    function render(exitingSlide, exitClass, enteringSlide, enteringClass) {
        window.clearTimeout(exitTimer);
        slides.forEach((slide, index) => {
            slide.classList.remove(
                "is-current",
                "is-prev",
                "is-next",
                "is-exiting-left",
                "is-exiting-right",
                "is-new-side-left",
                "is-new-side-right",
            );
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

        if (exitingSlide && exitClass) {
            exitingSlide.classList.add(exitClass);
            exitTimer = window.setTimeout(() => {
                exitingSlide.classList.remove(exitClass);
            }, 300);
        }

        if (enteringSlide && enteringClass) {
            enteringSlide.classList.add(enteringClass);
            window.requestAnimationFrame(() => {
                window.requestAnimationFrame(() => {
                    enteringSlide.classList.remove(enteringClass);
                });
            });
        }
    }

    function showPrevious() {
        const exitingSlide = slides[(current + 1) % slides.length];
        const enteringSlide =
            slides.length > 2
                ? slides[(current - 2 + slides.length) % slides.length]
                : null;
        current = (current - 1 + slides.length) % slides.length;
        render(
            exitingSlide,
            "is-exiting-right",
            enteringSlide,
            "is-new-side-left",
        );
    }

    function showNext() {
        const exitingSlide = slides[(current - 1 + slides.length) % slides.length];
        const enteringSlide =
            slides.length > 2 ? slides[(current + 2) % slides.length] : null;
        current = (current + 1) % slides.length;
        render(
            exitingSlide,
            "is-exiting-left",
            enteringSlide,
            "is-new-side-right",
        );
    }

    const clickSound = new Audio("/static/click.mp3");
    clickSound.preload = "auto";
    clickSound.volume = 0.5;

    function playClick() {
        clickSound.currentTime = 0;
        clickSound.play().catch(() => {});
    }

    let clickCount = 0;
    function navigate(direction) {
        clickCount++;
        if (clickCount % 100 === 0) {
            randomScream.currentTime = 0;
            randomScream.play().catch(() => {});
        } else {
            playClick();
        }

        if (direction === "previous") {
            showPrevious();
        } else {
            showNext();
        }
    }

    prevBtn?.addEventListener("click", () => navigate("previous"));
    nextBtn?.addEventListener("click", () => navigate("next"));

    let lastWheelNav = 0;
    const WHEEL_NAV_DEBOUNCE = 100;
    carousel?.addEventListener(
        "wheel",
        event => {
            if (!event.deltaY) {
                return;
            }

            event.preventDefault();

            const now = performance.now();
            if (now - lastWheelNav < WHEEL_NAV_DEBOUNCE) {
                return;
            }
            lastWheelNav = now;

            navigate(event.deltaY < 0 ? "previous" : "next");
        },
        { passive: false },
    );

    slides.forEach(slide => {
        slide.addEventListener("click", event => {
            if (slide.classList.contains("is-prev")) {
                event.preventDefault();
                navigate("previous");
            } else if (slide.classList.contains("is-next")) {
                event.preventDefault();
                navigate("next");
            }
        });
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
    initLogoAnimation();
    initOutboundLinkTracking();
    initListPreviewCursor();
    initViewToggle();
    initCarousel();
});
