# ADR 0001: Understanding first, not only findings, for whole-repository review

- **Status:** Accepted (phase 1 implemented: `/co-review:understand`)
- **Date:** 2026-10-30
- **Details:** [the proposal](../proposals/understanding-a-repository.md) ·
  [the method as pseudocode](../proposals/understanding-a-repository.pseudocode.md) ·
  [the engine design](../proposals/understanding-engine.md)

## Context

Co-Review exists so a person can read a whole repository with the agent that built it and make it theirs again.
Its whole-repository review (`/co-review:audit`) is classical code review: the repository split into 4–10 areas, at
most three risk findings per area, an overview page with one flowchart, and coverage measured as files marked viewed.
That says what's wrong. It doesn't say what's there: after reading it a person still can't say what each folder is
for, what the domain words mean, how a request flows, which patterns and naming the code follows, or where to look
for a given thing.

Program-comprehension research is consistent on what is missing. Programmers build two models, of the code and of
the domain it serves, and the second takes deliberate effort (Pennington). They work by forming hypotheses and
checking them, top-down and bottom-up (Brooks; Letovsky; von Mayrhauser & Vans). Their hardest questions are about
relationships, reachability and rationale (Sillito et al.; LaToza & Myers; Ko et al.), and newcomers' biggest
obstacle is tacit knowledge: conventions, vocabulary, "where to start" (Dagenais et al.; Steinmacher et al.).
Generated documentation is preferred by readers, but preference isn't understanding, and passive reading of LLM
explanations produces an illusion of competence (Prather et al.).

## Decision

1. **Define understanding as nine checkable abilities** (U1–U9): purpose and boundary; the map; vocabulary and data;
   mechanics; rationale; patterns, naming and conventions; health and risk; acting (placing a change and predicting
   its blast radius); currency (knowing whether your picture is still true). Familiarity, recognising names, is not
   the target; prediction is.
2. **Add `/co-review:understand`** (`/co-review-understand` in Pi and Oh My Pi): the repository review's overview page
   becomes a guide in four chapters, the lay of the land, how it works, patterns, naming and why, and where it hurts,
   followed by the agent's findings. Each chapter ends with one optional prediction question the person answers in a
   comment.
3. **Patterns are described in the code's own terms**, not textbook labels, together with how things are named, what
   the names signal, and where to look for common tasks.
4. **Truth over fluency**: every claim links to its code; structural claims come from the repository map and graph,
   not from the model; unverified claims are tagged inferred or unknown, and an unknown "why" is asked, never
   invented. The guide names the commit it was written for.
5. **Keep `/co-review:audit`** as the quick findings-first pass. Understanding is a separate command, not a change to
   the audit's behaviour, so neither surprises the other's users.
6. **Phase 1 uses only what exists** (the overview page, threads, findings); in-place surfaces (explorer notes,
   glossary hovers, editor tours, progress per ability) and "since you last read this" come later.
7. **Document the method as language-neutral pseudocode**, separating deterministic steps from model steps, so it can
   be ported (e.g. to Rust) for speed.

## Consequences

- A whole-repository review takes the agent longer to prepare (the pilot's guide took about twelve minutes), and the
  overview page is much longer. Chapter 1 alone is meant to be useful on its own.
- The overview page now carries a document of its own; comments on it reach the agent with `target: "overview"`,
  which is how check answers and corrections arrive. No app change was needed.
- Quality depends on the model following the truthfulness rules. The evaluation harness in the pseudocode (held-out
  questions, a guide-only reader, a grader with the code) is how to tell.
- The guide lives in the review's overview, outside the repository, like every other review state.

## Alternatives considered

- **Change `/co-review:audit` itself.** Rejected for now: it would make the existing command slower and different
  for everyone who uses it for risks. Revisit once the trial shows which people prefer.
- **A separate knowledge bundle (e.g. OKF) as the guide.** Rejected: a second artifact to switch to, and bundle pages
  open as a review of their own, without the repository's code and findings beside them.
- **Eight chapters, one per ability.** Rejected after critique: too much at once; four chapters chunk it.
- **A scored quiz.** Rejected: developers skip or resent it; one optional prediction per chapter, answered like a
  colleague's question, keeps the benefit.

## Evidence

A pilot guide written for Co-Review itself was read by a model that had never seen the code, which answered 26
questions closed-book (12 written by the guide's author, 14 written independently); a grader with the code marked 20
correct, 6 partial and 0 wrong, and found 13 of 15 sampled guide claims accurate and 2 partly accurate. The reader was
a model, not a person; the phase 1 trial with people is the real test.

## References

Program comprehension

- R. Brooks. Towards a theory of the comprehension of computer programs. *International Journal of Man-Machine
  Studies*, 1983.
- E. Soloway, K. Ehrlich. Empirical studies of programming knowledge. *IEEE TSE*, 1984.
- N. Pennington. Stimulus structures and mental representations in expert comprehension of computer programs.
  *Cognitive Psychology*, 1987.
- S. Letovsky. Cognitive processes in program comprehension. *Journal of Systems and Software*, 1987.
- A. von Mayrhauser, A. M. Vans. Program comprehension during software maintenance and evolution. *IEEE Computer*,
  1995.
- M.-A. Storey. Theories, tools and research methods in program comprehension: past, present and future. *Software
  Quality Journal*, 2006.

Information needs and navigation

- J. Sillito, G. C. Murphy, K. De Volder. Questions programmers ask during software evolution tasks. *FSE*, 2006.
  https://doi.org/10.1145/1181775.1181779
- A. J. Ko, R. DeLine, G. Venolia. Information needs in collocated software development teams. *ICSE*, 2007.
- T. D. LaToza, B. A. Myers. Developers ask reachability questions. *ICSE*, 2010.
- D. Piorkowski, S. D. Fleming et al. Information foraging theory in software engineering. *CHI* 2013; *IEEE TSE*.
  https://web.engr.oregonstate.edu/~burnett/Reprints/TSE-IFT-2013-asprinted.pdf

Newcomers and onboarding

- B. Dagenais, H. Ossher, R. Bellamy, M. Robillard, J. de Vries. Moving into a new software project landscape.
  *ICSE*, 2010. https://www.cs.mcgill.ca/~martin/papers/icse2010.pdf
- I. Steinmacher, M. A. Graciotto Silva, M. A. Gerosa, D. F. Redmiles. A systematic literature review on the
  barriers faced by newcomers to open source software projects. *IST*, 2015. https://ime.usp.br/~gerosa/papers/IST.pdf
- D. Nam, A. Macvean, V. Hellendoorn, B. Vasilescu, B. Myers. Using an LLM to help with code understanding. *ICSE*,
  2024. https://arxiv.org/abs/2307.08177

Architecture, concepts and conventions

- G. C. Murphy, D. Notkin, K. Sullivan. Software reflexion models. *FSE* 1995; *IEEE TSE* 2001.
  https://www.cs.ubc.ca/~murphy/papers/rm/fse95.html
- T. Biggerstaff, B. Mitbander, D. Webster. The concept assignment problem in program understanding. *ICSE*, 1993/1994.
- J. Garcia, I. Ivkovic, N. Medvidovic. A comparative analysis of software architecture recovery techniques. *ASE*,
  2013.
- F. Deissenboeck, M. Pizka. Concise and consistent naming. *Software Quality Journal*, 2006.
- M. Allamanis, E. T. Barr, C. Bird, C. Sutton. Learning natural coding conventions (NATURALIZE). *FSE*, 2014.
  https://arxiv.org/abs/1402.4182

Evolution and health

- A. Tornhill. *Your Code as a Crime Scene*. Pragmatic Bookshelf, 2015.
- C. Bird, N. Nagappan, B. Murphy, H. Gall, P. Devanbu. Don't touch my code! Examining the effects of ownership on
  software quality. *FSE*, 2011.

Visualisation and guided reading

- R. Wettel, M. Lanza. Visualizing software systems as cities (CodeCity). *VISSOFT*, 2007.
- Microsoft. CodeTour (VS Code extension), 2020.

LLM-era repository documentation and comprehension

- Q. Luo et al. RepoAgent: an LLM-powered open-source framework for repository-level code documentation generation.
  *EMNLP* (demo), 2024. https://arxiv.org/abs/2402.16667
- D. Yang et al. DocAgent: a multi-agent system for automated code documentation generation. *ACL* (demo), 2025.
  https://arxiv.org/abs/2504.08725
- W. Sun et al. Commenting higher-level code unit: full code, reduced code, or hierarchical code summarization.
  2025. https://arxiv.org/abs/2503.10737
- RepoSummary: feature-oriented summarization of repositories. 2025. https://arxiv.org/abs/2510.11039
- SWE-QA: can language models answer repository-level code questions? 2025. https://arxiv.org/abs/2509.14635
- F. Liu et al. Exploring and evaluating hallucinations in LLM-powered code generation. 2024.
  https://arxiv.org/abs/2404.00971
- J. Prather et al. The robots are here: navigating the generative AI revolution in computing education, and
  follow-up studies on novices' over-estimated understanding when using LLMs. 2023–2024.

Citations with a link were checked against the source during the research; the others were checked through
secondary references only.
