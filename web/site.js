(function () {

    // Add/remove entries here and every page picks it up automatically —
    // this is the single source of truth for site navigation.
    const NAV_ITEMS = [
        { href: "/", label: "Home", match: p => p === "/" || p === "" || p.endsWith("/index.html") },
        { href: "/market", label: "Market", match: p => p.startsWith("/market") },
        { href: "/api-docs", label: "API", match: p => p.startsWith("/api-docs") },
        { href: "/verify", label: "Certificates", match: p => ["/verify", "/check", "/shadow"].some(s => p.startsWith(s)) },
        { href: "/security-policy", label: "Политика безопасности", match: p => p === "/security-policy" },
    ];

    function buildHeader() {

        const path = window.location.pathname;

        const header = document.createElement("header");
        header.className = "site-header";

        header.innerHTML = `
            <div class="nav-inner">

                <a href="/" class="nav-brand">
                    <span class="logo-dot"></span>
                    Pig-6
                </a>

                <nav class="nav-links" id="navLinks">
                    ${NAV_ITEMS.map(item =>
            `<a href="${item.href}"${item.match(path) ? ' class="active"' : ""}>${item.label}</a>`
        ).join("")}
                </nav>

                <button class="nav-toggle" id="navToggle" type="button" aria-label="Toggle menu">
                    <span></span><span></span><span></span>
                </button>

            </div>
        `;

        // Pages mark where the header should live with <div id="site-header"></div>.
        // Falls back to prepending it, so the script is safe even without that hook.
        const mountPoint = document.getElementById("site-header");

        if (mountPoint) {
            mountPoint.replaceWith(header);
        } else {
            document.body.prepend(header);
        }

        const toggle = document.getElementById("navToggle");
        const links = document.getElementById("navLinks");

        toggle.addEventListener("click", () => {
            links.classList.toggle("open");
            toggle.classList.toggle("open");
        });

        links.querySelectorAll("a").forEach(a => {
            a.addEventListener("click", () => {
                links.classList.remove("open");
                toggle.classList.remove("open");
            });
        });

        const onScroll = () => header.classList.toggle("scrolled", window.scrollY > 6);
        onScroll();
        window.addEventListener("scroll", onScroll, { passive: true });

    }

    function initReveal() {

        const items = document.querySelectorAll(".reveal");

        if (!items.length)
            return;

        if (!("IntersectionObserver" in window)) {
            items.forEach(el => el.classList.add("in-view"));
            return;
        }

        const observer = new IntersectionObserver(entries => {

            entries.forEach(entry => {

                if (entry.isIntersecting) {
                    entry.target.classList.add("in-view");
                    observer.unobserve(entry.target);
                }

            });

        }, { threshold: .15 });

        items.forEach(el => observer.observe(el));

    }

    function initRipples() {

        document.addEventListener("click", e => {

            const target = e.target.closest(".ripple");

            if (!target)
                return;

            const rect = target.getBoundingClientRect();
            const size = Math.max(rect.width, rect.height) * 1.6;

            const circle = document.createElement("span");
            circle.className = "ripple-circle";
            circle.style.width = circle.style.height = `${size}px`;
            circle.style.left = `${e.clientX - rect.left - size / 2}px`;
            circle.style.top = `${e.clientY - rect.top - size / 2}px`;

            target.appendChild(circle);

            circle.addEventListener("animationend", () => circle.remove());

        });

    }

    document.addEventListener("DOMContentLoaded", () => {
        buildHeader();
        initReveal();
        initRipples();
    });

})();
