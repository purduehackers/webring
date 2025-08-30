function setFlip(flip) {
    document.body.style.transform = flip ? "scaleX(-1)" : ""
    const links = document.getElementsByTagName("a")
    for (const link of links) {
        if (flip) {
            link.href = "/flip?url=" + encodeURIComponent(link.href)
        } else {
            const url = new URL(link.href)
            if (url.pathname === "/flip") {
                link.href = url.searchParams.get("url")
            }
        }
    }
}
