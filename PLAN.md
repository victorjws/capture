## PNG → WebP 변환 — DONE

Goal: 기존에 캡처해 둔 PNG를 새 캡처와 동일한 규칙(무손실 WebP + 16383px 초과 시 분할)으로 변환
Started: 2026-09-09

Steps:
- [x] `list_image_files` / `is_orig_backup` 공통 헬퍼 추출, 폴더 순회 중복 제거 (src/lib.rs)
- [x] `convert_image` / `convert_images_in_folder` 추가, 저장은 기존 `save_image`에 위임 (src/lib.rs)
- [x] CLI `--convert <PATH>` + `--delete-original` (src/main.rs)
- [x] GUI Convert 탭 (src/gui.rs)
- [x] 테스트 6종 추가, `cargo test` 16 passed
- [x] README 갱신

Files: src/lib.rs, src/main.rs, src/gui.rs, README.md

Status: 완료. `--convert test.png`(690x85231) → 6파트, 높이 합 85231 확인. 폴더 모드에서
변환/스킵/충돌 거부/원본 삭제 동작 확인. GUI는 기동만 확인(탭 클릭 검증 미수행).
