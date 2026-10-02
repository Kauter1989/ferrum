# Safety, privacy and review

## Clinical safety
- FERRUM is research and engineering software, not a medical device.
- Never state a diagnosis, a likelihood of disease, or a recommendation
  for treatment.
- Describe:
  - **where:** plane, slice number, patient mm;
  - **what was measured:** value, unit, method, uncertainty.
- Take numbers from tools (`probe`, `stats`, `profile`, `measure`,
  `segment …`), never from grey values or by counting pixels.
- If the data does not fit the task, say so and stop. Examples: wrong
  modality, region not covered, warnings about voxel size.

## Provenance and review
- **Proposals:** everything you create is a *proposal* (`author: agent`,
  `status: proposed`) until a clinician confirms it. Say so in your
  answer and list the items (`review list`).
- **Protected items:** you may change or delete only items created by
  agents. Items drawn by people or proposed by engines are protected.
- **Harness review:** `review confirm` and `review reject` work only if
  the operator allows harness review. They always name the person who
  decided (`by`), and every decision goes to the audit log.
- **Engines:** results from engines marked `research_only` are for
  research use only. Say so.

## Privacy
- **Withheld:** outputs contain no patient names, IDs or birth dates.
  Study dates and accession numbers appear only with the operator's
  consent, and UIDs are pseudonymised (`anon-…`). Do not try to obtain
  identifiers in other ways. Some images contain burned-in text; do not
  transcribe it.
- **Read-only sources:** source data is read-only. All results stay in
  the workspace.
- **Operator configuration:** the operator configures FERRUM in
  `ferrum-agent.toml`. It sets the readable folders, the workspace root,
  privacy, limits and harness review. A `forbidden` error means the
  configuration does not allow the call; tell the user instead of
  working around it.
