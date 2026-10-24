// Distributed Proofreaders client application script

document.addEventListener("DOMContentLoaded", () => {
    // Configure HTMX to automatically attach CSRF token to all requests
    document.body.addEventListener("htmx:configRequest", (event) => {
        const token = document.querySelector('meta[name="csrf-token"]')?.getAttribute("content");
        if (token) {
            event.detail.headers["X-CSRF-Token"] = token;
        }
    });

    console.debug("DProofreaders VN initialized.");
});
