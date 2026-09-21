## AGPL-3.0 전환 + 히스토리 재작성 — DONE

Goal: 라이선스를 AGPL-3.0으로 확정하고, 어느 커밋을 체크아웃해도 LICENSE가 존재하도록
히스토리를 2커밋으로 재작성
Started: 2026-09-21

Steps:
- [x] MIT vs AGPL 비교 → `AGPL-3.0-only` 확정 (데스크톱 앱이라 §13은 사실상 비활성,
      실효는 GPL-3.0과 동일 + SaaS 래핑 차단 선언)
- [x] 의존성 호환 확인 — image/egui/clap 등 MIT OR Apache-2.0, xcap Apache-2.0
      (AGPL 작품에 포함 가능, 단방향). ffmpeg는 별도 프로세스라 무관
- [x] 원본 69커밋 백업 — `backup/pre-agpl` 브랜치 + `pre-agpl-20260921` 태그 (9487b4f)
- [x] README `## License` 교체, 번들 폰트는 OFL 1.1로 AGPL 적용 대상 아님을 명시
- [x] Cargo.toml `license = "AGPL-3.0-only"` 추가
- [x] src/*.rs 6개에 SPDX 2줄 헤더 추가 (파일 단위로 복사돼도 라이선스가 따라가도록)
- [x] `git commit-tree`로 2커밋 재작성 — C1은 LICENSE 단일 트리, C2는 나머지 전부
- [x] `cargo fmt` + `cargo test` 33 passed

Files: LICENSE, README.md, Cargo.toml, src/lib.rs, src/main.rs, src/gui.rs,
src/constants.rs, src/presets.rs, src/bin/capture-gui.rs

Status: 완료. 기존 69커밋 중 56개가 `f`/`feat`/`ㄹ` 수준의 메시지라 보존 가치가 없다고
보고 전량 squash. 날짜는 원본 루트(2025-12-18)와 최신 커밋(2026-09-21) author date를
유지. origin/main 푸시는 미실행 — `git push --force-with-lease origin main`은 직접
실행해야 하고, 그 전까지 원격에는 MIT 시절 히스토리가 그대로 남아 있다. 이미 클론/포크한
쪽의 MIT 버전은 회수 불가하므로 재작성 효력은 이후 배포분에만 적용된다.
