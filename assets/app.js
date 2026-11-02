// Distributed Proofreaders client application script.
//
// This file must stay compatible with a strict Content-Security-Policy:
// no inline scripts, no inline event handlers, and no eval-based helpers.

/**
 * Announce a short message through the global screen-reader live region.
 * @param {string} message
 */
function announce(message) {
    const region = document.getElementById("app-status");
    if (!region) return;
    region.textContent = "";
    // Force a new announcement even when the text is unchanged.
    window.setTimeout(() => {
        region.textContent = message;
    }, 50);
}

document.addEventListener("DOMContentLoaded", () => {
    // Configure HTMX to automatically attach the CSRF token to all requests.
    document.body.addEventListener("htmx:configRequest", (event) => {
        const token = document
            .querySelector('meta[name="csrf-token"]')
            ?.getAttribute("content");
        if (token) {
            event.detail.headers["X-CSRF-Token"] = token;
        }
    });

    // Confirm destructive actions declared with data-confirm="...".
    // Capture phase so the confirmation runs before HTMX handles the submit.
    document.addEventListener(
        "submit",
        (event) => {
            if (!(event.target instanceof Element)) return;
            const form = event.target.closest("form[data-confirm]");
            if (!form) return;
            if (!window.confirm(form.dataset.confirm)) {
                event.preventDefault();
                event.stopPropagation();
            }
        },
        true
    );

    // Reset forms after a successful HTMX request when requested.
    document.body.addEventListener("htmx:afterRequest", (event) => {
        const elt = event.detail.elt;
        if (event.detail.successful && elt?.matches?.("[data-reset-on-success]")) {
            elt.reset();
        }
    });

    // Surface HTMX failures in the live region instead of failing silently.
    document.body.addEventListener("htmx:responseError", (event) => {
        const status = event.detail.xhr?.status ?? "?";
        announce(`Yêu cầu thất bại (mã ${status}). Vui lòng thử lại.`);
    });
    document.body.addEventListener("htmx:sendError", () => {
        announce("Không kết nối được máy chủ. Vui lòng kiểm tra mạng và thử lại.");
    });

    console.debug("DProofreaders VN initialized.");
});
