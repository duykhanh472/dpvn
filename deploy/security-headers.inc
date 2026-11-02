# Shared security headers. Included from every location that defines its own
# Cache-Control header, because nginx's add_header inheritance is not additive.
#
# The script policy deliberately has no 'unsafe-inline' and no 'unsafe-eval':
# all JavaScript is served from /static and all handlers are attached from
# external files (§21.2 acceptance criterion).

add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
add_header X-Content-Type-Options "nosniff" always;
add_header Referrer-Policy "same-origin" always;
add_header X-Frame-Options "DENY" always;
add_header Permissions-Policy "camera=(), microphone=(), geolocation=(), payment=(), usb=()" always;
add_header Cross-Origin-Opener-Policy "same-origin" always;
add_header Content-Security-Policy "default-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; object-src 'none'; img-src 'self' data:; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self'" always;
