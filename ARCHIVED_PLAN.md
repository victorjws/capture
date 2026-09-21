## JPEG XL 기본 출력 전환 — DONE

Goal: CBZ 장기 보관을 위해 무손실 JPEG XL을 기본 출력으로 삼고, 기존 PNG/WebP 캡처본을
안전하게 마이그레이션
Started: 2026-09-21

Steps:
- [x] 실측: test.png(690x85231) 기준 WebP 28.51MiB → JXL -e7 19.82MiB (-30.5%), 7.8초
      (-e8 -1.4%/18.3s, -e9 -2.3%/33.7s → `JXL_EFFORT = 7` 채택)
- [x] `encode_jxl`/`decode_jxl`/`open_image`/`write_pnm_temp`/`TempFile` 추가 (src/lib.rs)
- [x] `save_image` 확장자 분기화 (`max_part_height`, `write_one`), 실패 시 부분 출력 정리
- [x] `save_image_or_webp` 분리 — 폴백은 캡처 저장에만. 변환 폴백은 `a.webp`를
      자기 자신으로 덮어쓸 수 있어 금지
- [x] `validate_encoder` 프리플라이트 + spawn 실패 시 설치 안내(`JXL_TOOLS_HINT`)
- [x] 기본값 jxl 전환 (constants.rs OUTPUT_PATH, main.rs --format, gui.rs config)
- [x] `--delete-original` 검증 후 삭제 (`verify_written_matches`)
- [x] `convert_images_in_folder` 반환값 `(converted, skipped, failed)` 분리 + 호출부 갱신
- [x] 테스트 17종 추가, `cargo test` 33 passed
- [x] README 갱신, 이전 플랜 ARCHIVED_PLAN.md로 이동

Files: src/lib.rs, src/main.rs, src/gui.rs, src/constants.rs, README.md

Status: 완료. test.png → 6파트 JXL 19.79MiB, 6파트를 이어 붙인 raw RGBA가 원본과
바이트 일치(235,237,560 bytes). JXL→WebP 재변환 결과가 PNG→WebP 직접 변환과 파트별
바이트 동일. 혼합 폴더(png/webp/jxl/깨진 파일)에서 (2,1,1) 집계 및 검증 통과분만
삭제 확인. PATH에서 cjxl 제거 시 캡처 시작 전 안내 에러 확인. macOS sips가 .jxl을
네이티브 디코드하므로 Preview 계열 뷰어에서 CBZ 열람 가능. GUI는 기동만 확인
(위젯 클릭 검증 미수행).

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
