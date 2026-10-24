-- ============================================================
-- 0001_init.sql
-- ============================================================

CREATE TABLE users (
  id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  username       VARCHAR(32)  NOT NULL,
  email          VARCHAR(255) NOT NULL,
  password_hash  VARCHAR(255) NOT NULL,
  real_name      VARCHAR(100) NOT NULL DEFAULT '',
  status         ENUM('pending','active','disabled') NOT NULL DEFAULT 'pending',
  activate_token CHAR(43)     NULL,
  created_at     DATETIME(6)  NOT NULL,
  last_login_at  DATETIME(6)  NULL,
  UNIQUE KEY users_username (username),
  UNIQUE KEY users_email    (email),
  KEY users_status (status)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

ALTER TABLE users
  MODIFY username VARCHAR(32)  COLLATE utf8mb4_0900_ai_ci NOT NULL,
  MODIFY email    VARCHAR(255) COLLATE utf8mb4_0900_ai_ci NOT NULL;

CREATE TABLE user_roles (
  user_id    BIGINT UNSIGNED NOT NULL,
  role       ENUM('proofer','pm','pp','admin') NOT NULL,
  granted_at DATETIME(6) NOT NULL,
  PRIMARY KEY (user_id, role),
  CONSTRAINT fk_roles_user FOREIGN KEY (user_id)
    REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE sessions (
  token_hash   BINARY(32) PRIMARY KEY,
  user_id      BIGINT UNSIGNED NOT NULL,
  csrf_token   CHAR(43) NOT NULL,
  created_at   DATETIME(6) NOT NULL,
  last_seen_at DATETIME(6) NOT NULL,
  expires_at   DATETIME(6) NOT NULL,
  ip           VARBINARY(16) NULL,
  user_agent   VARCHAR(255) NOT NULL DEFAULT '',
  KEY sessions_user   (user_id),
  KEY sessions_expiry (expires_at),
  CONSTRAINT fk_sess_user FOREIGN KEY (user_id)
    REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE projects (
  id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  name          VARCHAR(255) NOT NULL,
  author        VARCHAR(255) NOT NULL DEFAULT '',
  phase         ENUM('draft','queued','in_round','post_processing','posted','archived')
                  NOT NULL DEFAULT 'draft',
  current_round VARCHAR(8)  NULL,
  on_hold       TINYINT(1)  NOT NULL DEFAULT 0,
  tone_style    ENUM('old','new','either') NOT NULL DEFAULT 'either',
  charset       VARCHAR(32) NOT NULL DEFAULT 'vi',
  manager_id    BIGINT UNSIGNED NOT NULL,
  image_source  VARCHAR(64) NOT NULL DEFAULT '',
  comments      LONGTEXT NOT NULL,
  created_at    DATETIME(6) NOT NULL,
  updated_at    DATETIME(6) NOT NULL,
  KEY projects_pool    (phase, current_round, on_hold),
  KEY projects_manager (manager_id),
  CONSTRAINT fk_proj_manager FOREIGN KEY (manager_id) REFERENCES users(id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE pages (
  id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id   BIGINT UNSIGNED NOT NULL,
  page_number  INT NOT NULL,
  image_path   VARCHAR(255) NOT NULL,
  image_width  INT UNSIGNED NOT NULL DEFAULT 0,
  image_height INT UNSIGNED NOT NULL DEFAULT 0,
  ocr_text     LONGTEXT NULL,
  UNIQUE KEY pages_order (project_id, page_number),
  CONSTRAINT fk_pages_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_rounds (
  page_id    BIGINT UNSIGNED NOT NULL,
  round      VARCHAR(8) NOT NULL,
  state      ENUM('available','in_progress','done') NOT NULL DEFAULT 'available',
  text       LONGTEXT NULL,
  proofer_id BIGINT UNSIGNED NULL,
  claimed_at DATETIME(6) NULL,
  done_at    DATETIME(6) NULL,
  PRIMARY KEY (page_id, round),
  KEY pr_pool (round, state),
  KEY pr_proofer (proofer_id, round),
  CONSTRAINT fk_pr_page FOREIGN KEY (page_id)
    REFERENCES pages(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_versions (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  page_id    BIGINT UNSIGNED NOT NULL,
  round      VARCHAR(8) NOT NULL,
  text       LONGTEXT NOT NULL,
  proofer_id BIGINT UNSIGNED NULL,
  created_at DATETIME(6) NOT NULL,
  KEY pv_lookup (page_id, round, created_at),
  CONSTRAINT fk_pv_page FOREIGN KEY (page_id)
    REFERENCES pages(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_events (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id BIGINT UNSIGNED NOT NULL,
  from_phase VARCHAR(24) NULL,
  to_phase   VARCHAR(24) NULL,
  from_round VARCHAR(8)  NULL,
  to_round   VARCHAR(8)  NULL,
  actor_id   BIGINT UNSIGNED NULL,
  note       VARCHAR(255) NOT NULL DEFAULT '',
  created_at DATETIME(6) NOT NULL,
  KEY pe_project (project_id, created_at),
  CONSTRAINT fk_pe_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE site_words (
  kind     ENUM('good','bad') NOT NULL,
  word     VARCHAR(64) NOT NULL,
  added_by BIGINT UNSIGNED NULL,
  added_at DATETIME(6) NOT NULL,
  PRIMARY KEY (kind, word)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_words (
  project_id BIGINT UNSIGNED NOT NULL,
  kind       ENUM('good','bad') NOT NULL,
  word       VARCHAR(64) NOT NULL,
  added_by   BIGINT UNSIGNED NULL,
  added_at   DATETIME(6) NOT NULL,
  PRIMARY KEY (project_id, kind, word),
  CONSTRAINT fk_pw_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_comments (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id BIGINT UNSIGNED NOT NULL,
  author_id  BIGINT UNSIGNED NOT NULL,
  body_md    TEXT NOT NULL,
  body_html  TEXT NOT NULL,
  created_at DATETIME(6) NOT NULL,
  KEY pc_project (project_id, created_at),
  CONSTRAINT fk_pc_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_tallies (
  user_id BIGINT UNSIGNED NOT NULL,
  round   VARCHAR(8) NOT NULL,
  day     DATE NOT NULL,
  count   INT UNSIGNED NOT NULL DEFAULT 0,
  PRIMARY KEY (user_id, round, day)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE settings (
  k          VARCHAR(64) PRIMARY KEY,
  v          TEXT NOT NULL,
  updated_at DATETIME(6) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;
