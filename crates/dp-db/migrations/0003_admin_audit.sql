-- ============================================================
-- 0003_admin_audit.sql
-- Audit trail for administrative actions that are not tied to a
-- project lifecycle transition (role changes, account status,
-- site settings, dictionary maintenance).
-- ============================================================

CREATE TABLE admin_audit_log (
  id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  actor_id       BIGINT UNSIGNED NULL,
  actor_username VARCHAR(32)  NOT NULL,
  action         VARCHAR(64)  NOT NULL,
  target_type    VARCHAR(32)  NOT NULL,
  target_id      BIGINT UNSIGNED NULL,
  detail         VARCHAR(500) NOT NULL DEFAULT '',
  created_at     DATETIME(6)  NOT NULL,
  KEY aal_created (created_at, id),
  KEY aal_actor   (actor_id, created_at),
  CONSTRAINT fk_aal_actor FOREIGN KEY (actor_id)
    REFERENCES users(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;
