# Frontend design

- UI 변경 전 [디자인 계약](../docs/design-system.md)을 읽는다.
- 색상·간격·타입·radius·motion은 `design-tokens.css`, 배치는 `styles.css`에 둔다. 로컬 팔레트를 만들지 않는다.
- HeroUI v3 실제 Button/Input/Checkbox를 사용하고 mutable control에는 명시적 잠금을 준다.
- 섹션은 제목·여백·구분선으로 구성한다. Card/Surface wrapper와 중첩 카드는 금지한다.
- primary는 화면 상태당 하나다. Rust PNG·두 hash·기존 인쇄/취소·입력 오류 계약을 유지한다.
- App은 표시/zoom만, usePrintWorkflow는 작업 상태/요청, native는 모든 Tauri IO, contracts는 DTO만 소유한다. 역방향 import와 새 파일은 architecture guard에서 검사한다.
- 작은 상단 Open과 desktop fit/Print 가시성, 가로·세로 중앙의 bounded dot viewport를 유지한다. 큰 원본은 먼저 불러오고 용지/이미지 크기를 조절한다. 잘못된 설정은 숨기지 않고 회복 경로를 보여준다. 작업별 async 상태와 stale 성공/실패 폐기를 함께 검사한다.
