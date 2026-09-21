## 분할된 파트 합치기 변환 — DONE

Goal: 높이 제한 때문에 `ch01_1.webp`, `ch01_2.webp` … 로 쪼개진 한 화를, 높이 제한이 없는
포맷(jxl/png 등)으로 변환할 때 `ch01.jxl` 한 파일로 합치기
Started: 2026-09-21

Steps:
- [x] `max_part_height`에서 JXL 상한 해제 — JXL은 한 파일, WebP만 16383 분할 유지 (src/lib.rs)
- [x] `part_number` / `sibling_parts` 추가 — `stem_NN.ext`가 1..N 빈틈없이 모인
      같은 확장자 묶음(2개 이상)만 인식
- [x] `merge_parts` 추가 — 파트를 위에서 아래로 이어 붙임. 폭이 다르면 애초에 분할본이
      아니므로 `Ok(None)`을 돌려 개별 변환으로 폴백
- [x] `convert_group` 추출 — 단일/묶음 공통 경로, 결과는 `Converted::{Written,
      AlreadyTarget, NotOneImage}`
- [x] `convert_image`는 묶음 우선 시도 후 단일로 폴백, `convert_images_in_folder`는
      묶음을 1건으로 집계 + 이미 합쳐진 파트 스킵 + 실패한 묶음 재시도 방지
- [x] `--delete-original`은 검증 통과 후 파트 전부 삭제
- [x] 테스트 11종 추가 + JXL 분할 테스트 2종 교체, `cargo fmt` + `cargo test` 44 passed
- [x] README / GUI Convert 탭 안내 문구 갱신

Files: src/lib.rs, src/gui.rs, README.md

Status: 완료. 실측 — test.png(690x85231) → WebP 6파트 → `--convert ch01_1.webp --format jxl`
결과가 PNG 직접 변환본과 **바이트 단위 동일**(19.8MiB, 690x85231 단일 파일), 디코드 픽셀도
동일. 폴더 모드에서 6파트 묶음 + 단독 png = 2건 집계, `--delete-original` 검증 후 파트
6개 전부 삭제 확인. GUI는 컴파일만 확인(위젯 클릭 미검증).

주의: JXL 상한 해제로 CBZ 페이지 분할은 이제 WebP 열람본 쪽에서만 일어난다. 기존에
16383으로 쪼개 둔 `*_N.jxl` 보관본은 `--convert <파일> --format jxl`로 합칠 수 있다
(같은 포맷이어도 파트 묶음이면 병합 대상).
