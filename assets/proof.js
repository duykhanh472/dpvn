import { EditorView, keymap, lineNumbers, highlightActiveLine, Decoration, ViewPlugin } from "@codemirror/view";
import { EditorState, StateField, StateEffect } from "@codemirror/state";
import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
import { search, searchKeymap } from "@codemirror/search";

// ─────────────────────────────────────────────────────────────────────────────
// UTF-8 Byte Offset to UTF-16 Code Unit Conversion (§9.4, §14.2)
// ─────────────────────────────────────────────────────────────────────────────
function byteToUtf16Offset(text, byteOffset) {
    const encoder = new TextEncoder();
    let low = 0;
    let high = text.length;

    // Fast path: text is pure ASCII
    let isAscii = true;
    for (let i = 0; i < Math.min(text.length, 100); i++) {
        if (text.charCodeAt(i) > 127) {
            isAscii = false;
            break;
        }
    }
    if (isAscii && byteOffset <= text.length) {
        return byteOffset;
    }

    // Binary search matching byte length
    while (low < high) {
        const mid = (low + high) >> 1;
        const sub = text.slice(0, mid);
        const bytes = encoder.encode(sub).length;
        if (bytes < byteOffset) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    return low;
}

// ─────────────────────────────────────────────────────────────────────────────
// CM6 Wordcheck Decoration Layer (§14.2)
// ─────────────────────────────────────────────────────────────────────────────
const setIssuesEffect = StateEffect.define();

const issueMark = Decoration.mark({
    class: "cm-word-issue",
    attributes: { title: "Cảnh báo chính tả / bộ ký tự" },
});

const issuesField = StateField.define({
    create() {
        return Decoration.none;
    },
    update(issues, tr) {
        issues = issues.map(tr.changes);
        for (const e of tr.effects) {
            if (e.is(setIssuesEffect)) {
                issues = e.value;
            }
        }
        return issues;
    },
    provide: (f) => EditorView.decorations.from(f),
});

// ─────────────────────────────────────────────────────────────────────────────
// Proofing Workspace Controller
// ─────────────────────────────────────────────────────────────────────────────
document.addEventListener("DOMContentLoaded", () => {
    const root = document.getElementById("proof-workspace");
    if (!root) return;

    const projectId = root.dataset.projectId;
    const pageNumber = parseInt(root.dataset.pageNumber, 10);
    const csrfToken = root.dataset.csrfToken || "";
    const isReadonly = root.dataset.readonly === "1";

    let isDirty = false;
    let editorView = null;
    let confirmedHash = null;

    // 1. Initialize CodeMirror 6
    const rawTextEl = document.getElementById("raw-initial-text");
    const initialText = rawTextEl ? rawTextEl.value : "";

    const saveKeymap = [
        {
            key: "Mod-s",
            run: () => {
                if (!isReadonly) savePage();
                return true;
            },
        },
    ];

    const updateListener = EditorView.updateListener.of((update) => {
        if (update.docChanged) {
            isDirty = true;
            updateStats();
            updateSaveStatus("Chưa lưu", "unsaved");
            if (!editorView.composing) {
                scheduleWordcheck();
            }
        }
    });

    const state = EditorState.create({
        doc: initialText,
        extensions: [
            lineNumbers(),
            highlightActiveLine(),
            history(),
            search(),
            EditorView.lineWrapping,
            EditorView.editable.of(!isReadonly),
            EditorView.contentAttributes.of({
                spellcheck: "false",
                lang: "vi",
                "aria-label": "Khu vực soạn thảo văn bản soát lỗi",
            }),
            issuesField,
            keymap.of([...saveKeymap, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
            updateListener,
        ],
    });

    const mountEl = document.getElementById("codemirror-mount");
    editorView = new EditorView({
        state,
        parent: mountEl,
    });

    // 2. IME Composition Guard (§8.6)
    // Proofers use Telex/VNI; never run wordcheck mid-composition
    editorView.contentDOM.addEventListener("compositionend", () => {
        // Debounce 400ms after composition completes
        setTimeout(() => {
            scheduleWordcheck();
        }, 400);
    });

    // 3. Stats update (characters and words)
    function updateStats() {
        const text = editorView.state.doc.toString();
        const charCount = Array.from(text).length; // grapheme/codepoint count
        const words = text.trim().split(/\s+/).filter(Boolean);
        const wordCount = text.trim() === "" ? 0 : words.length;

        const statChars = document.getElementById("stat-chars");
        const statWords = document.getElementById("stat-words");
        if (statChars) statChars.innerHTML = `Ký tự: <strong>${charCount}</strong>`;
        if (statWords) statWords.innerHTML = `Từ: <strong>${wordCount}</strong>`;
    }
    updateStats();

    function updateSaveStatus(text, className) {
        const el = document.getElementById("save-status");
        if (!el) return;
        el.textContent = text;
        el.className = `save-status-${className}`;
    }

    // 4. Character Palette Insertion
    document.querySelectorAll(".palette-btn").forEach((btn) => {
        btn.addEventListener("click", () => {
            if (isReadonly) return;
            const char = btn.dataset.char;
            if (!char) return;

            const tr = editorView.state.replaceSelection(char);
            editorView.dispatch(tr);
            editorView.focus();
        });
    });

    // 5. Unsaved Changes Guard
    window.addEventListener("beforeunload", (e) => {
        if (isDirty && !isReadonly) {
            e.preventDefault();
            e.returnValue = "Bạn có thay đổi chưa lưu trên trang này. Rời đi?";
            return e.returnValue;
        }
    });

    // 6. Realtime Wordcheck Checking (§9.6)
    let wordcheckTimeout = null;
    function scheduleWordcheck() {
        if (wordcheckTimeout) clearTimeout(wordcheckTimeout);
        wordcheckTimeout = setTimeout(runWordcheck, 400);
    }

    async function runWordcheck() {
        if (!editorView || editorView.composing) return;
        const text = editorView.state.doc.toString();

        try {
            const resp = await fetch(`/projects/${projectId}/pages/${pageNumber}/wordcheck`, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    "X-CSRF-Token": csrfToken,
                },
                body: JSON.stringify({ text }),
            });

            if (!resp.ok) return;
            const issues = await resp.json();

            // Rebuild CM6 decorations
            const decos = [];
            for (const issue of issues) {
                const from = byteToUtf16Offset(text, issue.byte_start);
                const to = byteToUtf16Offset(text, issue.byte_end);
                if (from < to && to <= text.length) {
                    decos.push(issueMark.range(from, to));
                }
            }

            decos.sort((a, b) => a.from - b.from);
            const decorationSet = Decoration.set(decos);
            editorView.dispatch({
                effects: setIssuesEffect.of(decorationSet),
            });

            const statIssues = document.getElementById("stat-issues");
            if (statIssues) {
                if (issues.length > 0) {
                    statIssues.className = "stat-issues-warn";
                    statIssues.innerHTML = `Chính tả: <strong>${issues.length} lưu ý</strong>`;
                } else {
                    statIssues.className = "stat-issues-clean";
                    statIssues.innerHTML = `Chính tả: <strong>Sạch</strong>`;
                }
            }
        } catch (err) {
            console.debug("Wordcheck preview skipped:", err);
        }
    }

    // 7. Save Page Flow (§9.5, §14.4)
    async function savePage(overrideHash = null) {
        if (isReadonly) return;
        const text = editorView.state.doc.toString();
        updateSaveStatus("Đang lưu...", "saving");

        const payload = {
            text,
            confirmed_hash: overrideHash || confirmedHash,
            csrf_token: csrfToken,
        };

        try {
            const resp = await fetch(`/projects/${projectId}/pages/${pageNumber}/save`, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    "X-CSRF-Token": csrfToken,
                },
                body: JSON.stringify(payload),
            });

            const contentType = resp.headers.get("content-type") || "";

            if (resp.status === 200 && contentType.includes("text/html")) {
                // Server returned confirmation panel HTML (§9.5 HTMX swap pattern)
                const html = await resp.text();
                const issuesWrapper = document.getElementById("issues-wrapper");
                if (issuesWrapper) {
                    issuesWrapper.innerHTML = html;
                    issuesWrapper.style.display = "block";

                    // Bind confirm actions
                    const cancelBtn = document.getElementById("btn-cancel-confirm");
                    const submitBtn = document.getElementById("btn-submit-confirm");

                    if (cancelBtn) {
                        cancelBtn.addEventListener("click", () => {
                            issuesWrapper.style.display = "none";
                            editorView.focus();
                        });
                    }

                    if (submitBtn) {
                        submitBtn.addEventListener("click", () => {
                            const hash = submitBtn.dataset.hash;
                            confirmedHash = hash;
                            savePage(hash);
                        });
                    }
                }
                updateSaveStatus("Cần xác nhận", "unsaved");
                return;
            }

            if (resp.ok) {
                const data = await resp.json();
                isDirty = false;
                updateSaveStatus("Đã lưu!", "saved");
                if (data.next_url) {
                    window.location.href = data.next_url;
                }
            } else {
                const errText = await resp.text();
                updateSaveStatus("Lỗi lưu", "error");
                alert(`Không thể lưu trang: ${errText || "Lỗi không xác định"}`);
            }
        } catch (err) {
            console.error("Save failed:", err);
            updateSaveStatus("Lỗi mạng", "error");
            alert("Lỗi kết nối máy chủ. Vui lòng thử lại.");
        }
    }

    const saveBtn = document.getElementById("btn-save-page");
    if (saveBtn) {
        saveBtn.addEventListener("click", () => savePage());
    }

    // 8. Draggable Split Resizing (§14.1)
    initSplitResize();

    // 9. Image Viewer (Zoom, Pan, Rotate)
    initImageViewer();
});

// ─────────────────────────────────────────────────────────────────────────────
// Draggable Percentage-Based Split (§14.1)
// ─────────────────────────────────────────────────────────────────────────────
function initSplitResize() {
    const splitContainer = document.getElementById("proof-split");
    const imagePane = document.getElementById("image-pane");
    const textPane = document.getElementById("text-pane");
    const handle = document.getElementById("split-handle");

    if (!splitContainer || !imagePane || !textPane || !handle) return;

    // Restore saved percentage (defaults to 50%)
    const saved = localStorage.getItem("dp_proof_split");
    let currentPct = saved ? parseFloat(saved) : 50;
    if (isNaN(currentPct) || currentPct < 20 || currentPct > 80) {
        currentPct = 50;
    }
    applySplit(currentPct);

    function applySplit(pct) {
        currentPct = Math.max(20, Math.min(80, pct));
        if (window.innerWidth >= 900) {
            imagePane.style.width = `${currentPct}%`;
            textPane.style.width = `${100 - currentPct}%`;
            imagePane.style.height = "100%";
            textPane.style.height = "100%";
        } else {
            imagePane.style.width = "100%";
            textPane.style.width = "100%";
            imagePane.style.height = `${currentPct}%`;
            textPane.style.height = `${100 - currentPct}%`;
        }
        localStorage.setItem("dp_proof_split", currentPct.toString());
    }

    let isDragging = false;

    handle.addEventListener("pointerdown", (e) => {
        isDragging = true;
        handle.setPointerCapture(e.pointerId);
        document.body.classList.add("is-resizing");
    });

    handle.addEventListener("pointermove", (e) => {
        if (!isDragging) return;
        const rect = splitContainer.getBoundingClientRect();
        if (window.innerWidth >= 900) {
            const pct = ((e.clientX - rect.left) / rect.width) * 100;
            applySplit(pct);
        } else {
            const pct = ((e.clientY - rect.top) / rect.height) * 100;
            applySplit(pct);
        }
    });

    const stopDrag = () => {
        if (isDragging) {
            isDragging = false;
            document.body.classList.remove("is-resizing");
        }
    };
    handle.addEventListener("pointerup", stopDrag);
    handle.addEventListener("pointercancel", stopDrag);

    // Keyboard support on separator (ArrowLeft/ArrowRight)
    handle.addEventListener("keydown", (e) => {
        if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
            applySplit(currentPct - 2);
            e.preventDefault();
        } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
            applySplit(currentPct + 2);
            e.preventDefault();
        }
    });

    window.addEventListener("resize", () => applySplit(currentPct));
}

// ─────────────────────────────────────────────────────────────────────────────
// Image Viewer: Pan, Zoom, Rotate (§14.1)
// ─────────────────────────────────────────────────────────────────────────────
function initImageViewer() {
    const viewport = document.getElementById("image-viewport");
    const stage = document.getElementById("image-stage");
    const img = document.getElementById("proof-image");

    if (!viewport || !stage || !img) return;

    let scale = 1.0;
    let rotation = 0; // 0, 90, 180, 270
    let posX = 0;
    let posY = 0;

    let isPanning = false;
    let startX = 0;
    let startY = 0;

    function updateTransform() {
        stage.style.transform = `translate(${posX}px, ${posY}px) rotate(${rotation}deg) scale(${scale})`;
    }

    function fitToWidth() {
        const vpWidth = viewport.clientWidth - 40;
        const imgWidth = img.naturalWidth || img.width || 800;
        scale = Math.max(0.2, Math.min(2.0, vpWidth / imgWidth));
        posX = 0;
        posY = 0;
        updateTransform();
    }

    if (img.complete) {
        fitToWidth();
    } else {
        img.addEventListener("load", fitToWidth);
    }

    // Zoom buttons
    document.getElementById("btn-zoom-in")?.addEventListener("click", () => {
        scale = Math.min(4.0, scale * 1.25);
        updateTransform();
    });

    document.getElementById("btn-zoom-out")?.addEventListener("click", () => {
        scale = Math.max(0.2, scale / 1.25);
        updateTransform();
    });

    document.getElementById("btn-zoom-100")?.addEventListener("click", () => {
        scale = 1.0;
        posX = 0;
        posY = 0;
        updateTransform();
    });

    document.getElementById("btn-zoom-200")?.addEventListener("click", () => {
        scale = 2.0;
        posX = 0;
        posY = 0;
        updateTransform();
    });

    document.getElementById("btn-zoom-fit")?.addEventListener("click", () => {
        fitToWidth();
    });

    document.getElementById("btn-rotate")?.addEventListener("click", () => {
        rotation = (rotation + 90) % 360;
        updateTransform();
    });

    // Mouse drag to pan
    viewport.addEventListener("pointerdown", (e) => {
        // Only left button pan
        if (e.button !== 0) return;
        isPanning = true;
        startX = e.clientX - posX;
        startY = e.clientY - posY;
        viewport.setPointerCapture(e.pointerId);
        viewport.style.cursor = "grabbing";
    });

    viewport.addEventListener("pointermove", (e) => {
        if (!isPanning) return;
        posX = e.clientX - startX;
        posY = e.clientY - startY;
        updateTransform();
    });

    const stopPan = () => {
        isPanning = false;
        viewport.style.cursor = "grab";
    };
    viewport.addEventListener("pointerup", stopPan);
    viewport.addEventListener("pointercancel", stopPan);

    // Mouse wheel zoom
    viewport.addEventListener("wheel", (e) => {
        e.preventDefault();
        const delta = e.deltaY < 0 ? 1.1 : 0.9;
        scale = Math.max(0.2, Math.min(4.0, scale * delta));
        updateTransform();
    }, { passive: false });

    // Global keyboard shortcuts (when not typing in editor)
    window.addEventListener("keydown", (e) => {
        const target = e.target;
        if (target.closest(".cm-content") || target.tagName === "INPUT" || target.tagName === "TEXTAREA") {
            return;
        }

        if (e.key === "+" || e.key === "=") {
            scale = Math.min(4.0, scale * 1.25);
            updateTransform();
            e.preventDefault();
        } else if (e.key === "-" || e.key === "_") {
            scale = Math.max(0.2, scale / 1.25);
            updateTransform();
            e.preventDefault();
        } else if (e.key === "0") {
            fitToWidth();
            e.preventDefault();
        } else if (e.key === "r" || e.key === "R") {
            rotation = (rotation + 90) % 360;
            updateTransform();
            e.preventDefault();
        }
    });
}
