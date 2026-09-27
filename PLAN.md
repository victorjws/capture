## 원본 손실 없는 JXL 변환 — DONE

Goal: `--format jxl` 변환에서 cjxl이 직접 읽는 원본은 디코드하지 않고 파일째 넘겨, JPEG은
트랜스코드(파일 기준 무손실), PNG는 16비트·그레이스케일, GIF는 전 프레임, ICC/EXIF/XMP는
컨테이너째 보존한다. 담을 수 없는 것이 생기면 조용히 버리지 않고 원본도 지우지 않는다
Started: 2026-09-26

Steps:
- [x] `encode_jxl_from_file` + `CJXL_DIRECT_INPUT` 추가 — `cjxl <원본> <출력> -d 0 -e 7`.
      `--allow_jpeg_reconstruction=0`과 `-x strip=`은 절대 넘기지 않는다 (src/lib.rs)
- [x] `convert_group` 분기 — jxl 출력 + 병합 아님 + 직행 가능 확장자면 파일째 넘기고,
      실패 시 경고 후 기존 픽셀 경로로 폴백. `IMAGE_EXTENSIONS`에 `gif` 추가 (src/lib.rs)
- [x] `SourceImage` / `open_source_image` 추가 — cjxl이 못 읽는 webp/bmp/tiff만. 네이티브
      심도 유지 + ICC/EXIF 추출, 애니메이션 WebP는 거부 (src/lib.rs)
- [x] `encode_with_metadata` / `write_one` / `write_dynamic` — 인코더를 직접 만들어
      `set_icc_profile` + `set_exif_metadata` 호출. EXIF를 못 담는 출력은 Orientation을
      픽셀에 굽는다 (src/lib.rs)
- [x] `write_scratch_for_cjxl` — 8비트 RGB/RGBA/Luma에 ICC가 없으면 PNM 빠른 경로 유지,
      그 밖은 16비트를 담는 임시 PNG. EXIF는 `-x exif=`로 전달 (src/lib.rs)
- [x] `--delete-original` 검증을 경로별로 교체 — JPEG은 바이트 비교, PNG는 네이티브 심도,
      애니메이션은 APNG 프레임 비교, 손실 발생 시 삭제 거부 (src/lib.rs)
- [x] `Proof::{FileBytes, Pixels}` — 경로마다 증명 수준이 다르므로 로그가 그 이상을
      주장하지 않게 한다. `--delete-original` 통과 조건은 변경 없음 (src/lib.rs, src/main.rs)
- [x] 테스트 12종 추가, `cargo fmt` + `cargo test` 56 passed (스킵 0)
- [x] README 갱신 — "무손실"의 기준 정의, 대상별 보존 범위 표, 삭제 거부 조건,
      XMP 한계, GUI trim 한계

Files: src/lib.rs, src/main.rs, README.md (src/gui.rs는 무변경)

Status: 완료. 원인은 `open_image`의 `to_rgba8()` 평탄화였다. 캡처 파이프라인이
`xcap::Monitor::capture_image()`가 주는 `RgbaImage`에 맞춰 쓰였고, 나중에 `--convert`가 그
함수들을 재사용하려고 타입을 맞추면서 16비트·메타데이터·프레임이 조용히 사라졌다. 더 나쁜 건
`verify_written_matches`가 그 깎인 버퍼를 기준으로 비교해 "Verified lossless"를 찍고
`--delete-original`이 원본을 지웠다는 점이다.

실측:
- JPEG 690x1600 150,261 → **120,682** bytes (-19.7%), `cmp`로 원본 바이트 동일 확인.
  같은 파일이 기존 픽셀 경로에서는 **378.5K**(원본의 2.58배)였다
- PNG 690x1600 849,694 → **432,860** (-49%)
- 16비트 PNG → jxl → djxl 왕복이 `16-bit/color RGB`로 복귀, 검증 후 원본 삭제
- 애니메이션 GIF 3프레임 유지(`jxlinfo` 3 Frame, djxl APNG 왕복 3프레임)
- 16비트 → webp `--delete-original`은 거부, 원본 유지 + 변환본 경로 안내
- 6파트 webp 병합 → test.jxl **19.8M**, 이전 실측치와 동일(회귀 없음)
- 폴더 일괄(gif 포함) 3건 변환, 실패 0
- webp→jxl 릴리스 1.61s (cjxl 단독 1.57s) — PNM 빠른 경로 유지 확인
- clippy 변경 전후 46건 동일, 신규 코드 영역 0건
- 로그 확인: JPEG은 `Verified byte for byte against a.jpg`,
  PNG은 `Verified pixels and carried metadata against b.png`

주의 1: 트랜스코드 결과는 JPEG의 양자화 테이블을 쓰는 VarDCT(lossy 모드) JXL이다. 원본 JPEG의
손실은 그대로 남으므로 "무손실 이미지"가 아니라 "JPEG 파일 기준 무손실"이다.

주의 2: 애니메이션 프레임의 지연 시간이 0이면 djxl이 한 장으로 합친다. 테스트 픽스처가
`Frame::from_parts`로 실제 지연을 주는 이유이고, 그런 입력은 검증에서 프레임 불일치로 걸려
삭제가 거부된다(안전한 방향).

주의 3: webp/bmp/tiff 픽셀 경로는 ICC와 EXIF만 옮긴다. image 크레이트가 XMP를 노출하지 않아
XMP는 사라지고 손실로 보고되지도 않는다 — README에 명시했고, RIFF 청크를 걸어 탐지하는 건
별도 판단 사항으로 남겼다.

주의 4: GUI의 trim/crop은 여전히 `open_image`(8비트 RGBA)를 쓴다. 16비트 파일을 trim하면
8비트로 내려가지만 `<name>_orig.<ext>` 백업이 남는다. README에 알려진 한계로 적었다.
