# 대한민국 학교현장 생활지도·교육활동보호 법령 RAG 데이터셋 확장판

## 파일 구성
- `kr_school_guidance_laws_rag_expanded.json`
  - 법령/훈령/고시별 묶음형 JSON
  - `records[]` 아래에 법규 단위 메타데이터 + `key_articles[]` 요약 청크가 들어 있습니다.
- `kr_school_guidance_laws_rag_expanded_flat.jsonl`
  - 조문·기준 단위 플랫 청크 JSONL
  - 벡터 DB, BM25, 하이브리드 검색에 바로 넣기 쉽습니다.
- `kr_school_guidance_laws_rag_expanded_README.md`
  - 확장판 구조 설명 문서입니다.

## 이번 확장판에서 추가한 법령/규정
- 교육기본법
- 초ㆍ중등교육법 시행규칙
- 학교생활기록 작성 및 관리지침
- 학교보건법
- 학교보건법 시행령
- 학교보건법 시행규칙
- 학교건강검사규칙
- 학교급식법
- 학교급식법 시행규칙
- 교육환경 보호에 관한 법률
- 청소년 보호법
- 개인정보 보호법
- 개인정보 보호법 시행령
- 감염병의 예방 및 관리에 관한 법률
- 마약류 관리에 관한 법률
- 성폭력범죄의 처벌 등에 관한 특례법
- 아동ㆍ청소년의 성보호에 관한 법률
- 장애인 등에 대한 특수교육법 시행령

## 현재 수록 규모
- 법령/규정 레코드 수: 36개
- 플랫 청크 수: 107개

## 이번 확장판이 특히 좋아진 질의 축
- **생활지도 정당화**
  - 교육기본법
  - 초ㆍ중등교육법 시행규칙
  - 학교생활기록 작성 및 관리지침
- **보건·감염·등교중지**
  - 학교보건법 / 시행령 / 시행규칙
  - 학교건강검사규칙
  - 감염병의 예방 및 관리에 관한 법률
- **급식·알레르기·영양상담**
  - 학교급식법 / 시행규칙
- **개인정보·사진·CCTV**
  - 개인정보 보호법 / 시행령
- **흡연·음주·유해약물·마약**
  - 청소년 보호법
  - 마약류 관리에 관한 법률
- **디지털 성사안**
  - 성폭력범죄의 처벌 등에 관한 특례법
  - 아동ㆍ청소년의 성보호에 관한 법률
- **특수교육 지원 절차**
  - 장애인 등에 대한 특수교육법 시행령
- **학교 주변 유해환경**
  - 교육환경 보호에 관한 법률

## 추천 사용법
1. **바로 RAG에 넣을 때**
   - `kr_school_guidance_laws_rag_expanded_flat.jsonl` 사용
2. **사람이 검수/수정하면서 운영할 때**
   - `kr_school_guidance_laws_rag_expanded.json` 사용
3. **질문-검색 연결 품질을 올릴 때**
   - `retrieval_boosters.concept_map`으로 동의어 확장 후 검색
4. **학교별 맞춤 답변을 만들 때**
   - 학교별 `학교생활규정(학칙)` 문서를 반드시 함께 인덱싱
   - 특히 `prohibited_items_or_substances`, `electronic_device_rule`, `cctv_or_recording_rule_if_any`, `health_and_attendance_restriction_rule_if_any` 필드를 별도 정리 권장

## 핵심 필드
- `official_name`: 공식 법령명
- `current_status_label`: 데이터셋 작성 시점 기준 시행 라벨
- `school_relevance`: 학교 현장과의 연결 설명
- `rag.aliases`, `rag.topical_tags`: 검색 확장용
- `key_articles[].legal_point`: 조문 핵심 취지
- `key_articles[].teacher_use_case`: 현장 적용 포인트
- `key_articles[].retrieval_text`: 임베딩/검색용 합성 텍스트

## 주의
- 이 데이터셋은 **원문 전재본**이 아니라 **RAG 친화적 요약·파라프레이즈 구조화본**입니다.
- 실제 답변 생성 시에는 `source_url`의 최신 현행 페이지를 다시 확인하는 흐름을 권장합니다.
- 학교별 학칙과 교육청 지침은 학교/지역마다 다를 수 있으니 반드시 별도 수집해 같이 넣는 편이 좋습니다.
