-- Wipe the app's data and load a spread of test tasks covering every case:
-- projects and unfiled, nesting 3 deep, descriptions, deadlines (past / today /
-- near / far), done and not-done, logged time at several tree levels, a
-- nested project, and one habit task per periodicity kind (every case
-- next_occurrence/missed_occurrences/complete_recurring_task handle).
--
--   sqlite3 ~/.local/share/uhatt/uhatt.db < scripts/seed-dev-data.sql
--
-- Timestamps are local naive time to match the app (see src/db NOW / TODAY).

PRAGMA foreign_keys = ON;

DELETE FROM time_entries;
DELETE FROM actions;
DELETE FROM tasks;
DELETE FROM projects;
DELETE FROM meta;

-- ---- Projects --------------------------------------------------------------
INSERT INTO projects (id, name, tracked, archived, created_at) VALUES
  ('p-web',      'Website Relaunch', 1, 0, datetime('now','localtime','-40 days')),
  ('p-personal', 'Personal',         0, 0, datetime('now','localtime','-40 days')),
  ('p-learn',    'Learning',         1, 0, datetime('now','localtime','-40 days')),
  ('p-habits',   'Habits',           0, 0, datetime('now','localtime','-5 days'));

-- A nested project (PR #51): "Fitness" lives under "Habits" in the sidebar.
INSERT INTO projects (id, name, tracked, archived, created_at, parent_project_id) VALUES
  ('p-fitness', 'Fitness', 0, 0, datetime('now','localtime','-5 days'), 'p-habits');

-- ---- Tasks ---------------------------------------------------------------
-- cols: id, project_id, parent_task_id, title, notes, deadline, tracked,
--       status, completed_at, sort_order, created_at
-- `sort_order` is unique within a sibling group (as the app maintains it);
-- roots share one group, so they are numbered globally 10, 20, 30, …
INSERT INTO tasks
  (id, project_id, parent_task_id, title, notes, deadline, tracked, status, completed_at, sort_order, created_at)
VALUES
-- Website Relaunch
 ('t-landing','p-web',NULL,'Landing page',
  'New hero, trimmed copy, and a working CTA. Ship before the launch date.',
  date('now','localtime','+5 days'),1,'todo',NULL,10,datetime('now','localtime','-30 days')),
 ('t-hero','p-web','t-landing','Hero section','',
  date('now','localtime','+2 days'),0,'todo',NULL,1,datetime('now','localtime','-28 days')),
 ('t-hero-copy','p-web','t-hero','Write hero copy','Punchy, one sentence.',
  NULL,0,'todo',NULL,1,datetime('now','localtime','-27 days')),
 ('t-hero-visual','p-web','t-hero','Design hero visual','',
  NULL,0,'done',datetime('now','localtime','-3 days'),2,datetime('now','localtime','-27 days')),
 ('t-nav','p-web','t-landing','Navigation redesign','',
  NULL,0,'todo',NULL,2,datetime('now','localtime','-25 days')),
 ('t-perf','p-web','t-landing','Performance pass',
  'Lighthouse is at 61. Target 90+. Images and font loading are the worst offenders.',
  date('now','localtime','-1 day'),0,'todo',NULL,3,datetime('now','localtime','-20 days')),
 ('t-blog','p-web',NULL,'Blog migration','',
  date('now','localtime','+45 days'),0,'todo',NULL,20,datetime('now','localtime','-22 days')),
 ('t-blog-export','p-web','t-blog','Export old posts','',
  NULL,0,'done',datetime('now','localtime','-6 days'),1,datetime('now','localtime','-21 days')),
 ('t-blog-import','p-web','t-blog','Import into the new CMS','',
  NULL,0,'todo',NULL,2,datetime('now','localtime','-21 days')),
 ('t-launch','p-web',NULL,'Launch checklist','DNS, analytics, redirects, status page.',
  date('now','localtime'),0,'todo',NULL,30,datetime('now','localtime','-10 days')),
-- Personal
 ('t-taxes','p-personal',NULL,'File taxes','Gather receipts first, then the online form.',
  date('now','localtime','+20 days'),0,'todo',NULL,40,datetime('now','localtime','-15 days')),
 ('t-gym','p-personal',NULL,'Gym routine','3x a week: push / pull / legs.',
  NULL,1,'todo',NULL,50,datetime('now','localtime','-35 days')),
 ('t-car','p-personal',NULL,'Car service','',
  date('now','localtime','-3 days'),0,'todo',NULL,60,datetime('now','localtime','-12 days')),
-- Learning
 ('t-ddia','p-learn',NULL,'Read "Designing Data-Intensive Applications"',
  'One chapter a week. Take notes in the wiki.',
  NULL,1,'todo',NULL,70,datetime('now','localtime','-36 days')),
 ('t-ddia-ch5','p-learn','t-ddia','Chapter 5: Replication','',
  NULL,0,'done',datetime('now','localtime','-9 days'),1,datetime('now','localtime','-30 days')),
 ('t-ddia-ch6','p-learn','t-ddia','Chapter 6: Partitioning','',
  NULL,0,'todo',NULL,2,datetime('now','localtime','-30 days')),
 ('t-rust','p-learn',NULL,'Finish the Rust book','',
  NULL,0,'done',datetime('now','localtime','-2 days'),80,datetime('now','localtime','-38 days')),
-- Unfiled
 ('t-groceries',NULL,NULL,'Buy groceries','Milk, eggs, coffee, whatever else is out.',
  date('now','localtime','+1 day'),0,'todo',NULL,90,datetime('now','localtime','-2 days')),
 ('t-call',NULL,NULL,'Call the plumber about the leak','',
  NULL,0,'todo',NULL,100,datetime('now','localtime','-1 day')),
 ('t-idea',NULL,NULL,'App idea: offline-first habit tracker',
  'Local SQLite, sync later. Streaks, a weekly heatmap, and a widget. See if anything like it exists first.',
  NULL,0,'todo',NULL,110,datetime('now','localtime','-4 days'));

-- ---- Time entries ------------------------------------------------------
-- cols: id, task_id, start_ts, end_ts, source, note, created_at
INSERT INTO time_entries (id, task_id, start_ts, end_ts, source, note, created_at) VALUES
 ('e1','t-hero',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-8 days','start of day','+9 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-8 days','start of day','+11 hours','+20 minutes'),
   'timer','',datetime('now','localtime','-8 days')),
 ('e2','t-hero',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-6 days','start of day','+14 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-6 days','start of day','+15 hours','+35 minutes'),
   'manual','',datetime('now','localtime','-6 days')),
 ('e3','t-hero-copy',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-5 days','start of day','+10 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-5 days','start of day','+10 hours','+50 minutes'),
   'timer','',datetime('now','localtime','-5 days')),
 ('e4','t-landing',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-3 days','start of day','+13 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-3 days','start of day','+13 hours','+25 minutes'),
   'timer','',datetime('now','localtime','-3 days')),
 ('e5','t-gym',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-7 days','start of day','+18 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-7 days','start of day','+19 hours','+10 minutes'),
   'manual','',datetime('now','localtime','-7 days')),
 ('e6','t-gym',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-4 days','start of day','+18 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-4 days','start of day','+19 hours','+5 minutes'),
   'manual','',datetime('now','localtime','-4 days')),
 ('e7','t-gym',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-1 days','start of day','+18 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-1 days','start of day','+19 hours'),
   'timer','',datetime('now','localtime','-1 days')),
 ('e8','t-ddia-ch5',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-10 days','start of day','+20 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-10 days','start of day','+21 hours','+40 minutes'),
   'timer','',datetime('now','localtime','-10 days')),
 ('e9','t-ddia-ch6',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-2 days','start of day','+20 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','-2 days','start of day','+21 hours','+15 minutes'),
   'timer','',datetime('now','localtime','-2 days')),
 ('e10','t-perf',
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','start of day','+9 hours'),
   strftime('%Y-%m-%dT%H:%M:%S','now','localtime','start of day','+9 hours','+45 minutes'),
   'timer','',datetime('now','localtime'));

-- ---- Habits (recurring tasks) ----------------------------------------------
-- One task per `domain::Periodicity` kind, each in the state that best shows
-- off what it's for. Mark any of these done in the app to see
-- `complete_recurring_task` regenerate it - the whole subtree moves to
-- Finished and a fresh copy takes its place with an advanced deadline.
INSERT INTO tasks
  (id, project_id, parent_task_id, title, notes, deadline, tracked, status, completed_at, sort_order, created_at, periodicity)
VALUES
-- On schedule, not due yet: due tomorrow, completing it early still lands
-- the fresh copy 2 days out from *this* deadline, not from today.
 ('t-hab-water','p-habits',NULL,'Water the plants','',
  date('now','localtime','+1 day'),0,'todo',NULL,120,datetime('now','localtime','-20 days'),
  '{"kind":"days","n":2}'),
-- Overdue by several occurrences: opening this shows a "missed" count, and
-- completing it jumps straight to the next upcoming occurrence rather than
-- spawning one task per missed cycle.
 ('t-hab-trash','p-habits',NULL,'Take out the trash','',
  date('now','localtime','-10 days'),0,'todo',NULL,130,datetime('now','localtime','-40 days'),
  '{"kind":"days","n":3}'),
-- Due exactly today - also shows up under "Due today".
 ('t-hab-clean','p-habits',NULL,'Deep clean the kitchen','',
  date('now','localtime'),0,'todo',NULL,140,datetime('now','localtime','-14 days'),
  '{"kind":"weeks","n":1}'),
-- Every 3 months, deadline on the 30th: 3 months later is February, so
-- completing this is the easiest way to see the month-length clamp
-- (`step_occurrence` landing on the 28th/29th, not overflowing into March).
 ('t-hab-insurance','p-habits',NULL,'Pay quarterly insurance',
  'Auto-pay is off for this one - do it manually.',
  '2026-11-30',0,'todo',NULL,150,datetime('now','localtime','-60 days'),
  '{"kind":"months","n":3}'),
-- Specific weekday (Monday only).
 ('t-hab-standup','p-habits',NULL,'Team standup notes','',
  date('now','localtime','weekday 1'),0,'todo',NULL,160,datetime('now','localtime','-20 days'),
  '{"kind":"weekdays","days":[1]}'),
-- Specific weekdays, several - and in the nested "Fitness" project, so
-- opening it also exercises the project-nesting sidebar (PR #51).
 ('t-hab-gym','p-fitness',NULL,'Gym: push / pull / legs','',
  date('now','localtime','weekday 1'),0,'todo',NULL,170,datetime('now','localtime','-20 days'),
  '{"kind":"weekdays","days":[1,3,5]}'),
-- First day of the month.
 ('t-hab-report','p-habits',NULL,'Submit monthly report','',
  date('now','localtime','start of month','+1 month'),0,'todo',NULL,180,datetime('now','localtime','-20 days'),
  '{"kind":"first_of_month"}'),
-- Last day of the month.
 ('t-hab-closebooks','p-habits',NULL,'Close the books','',
  date('now','localtime','start of month','+1 month','-1 day'),0,'todo',NULL,190,datetime('now','localtime','-20 days'),
  '{"kind":"last_of_month"}'),
-- No deadline yet - completing it falls back to "today" as the anchor
-- instead of failing or crashing on a NULL `deadline`.
 ('t-hab-meditate','p-habits',NULL,'Meditate','',
  NULL,0,'todo',NULL,200,datetime('now','localtime','-5 days'),
  '{"kind":"days","n":1}'),
-- A habit with subtasks - completing "Weekly review" regenerates the whole
-- 3-task subtree in one shot, not just this row. Neither child has its own
-- periodicity; both inherit it from this parent (`effective_periodicity`).
 ('t-hab-weekly','p-habits',NULL,'Weekly review','',
  date('now','localtime'),0,'todo',NULL,210,datetime('now','localtime','-14 days'),
  '{"kind":"weeks","n":1}'),
-- Already done ahead of the parent - completing "Weekly review" (and later
-- reverting it) should leave this one done throughout, not flip it back.
 ('t-hab-weekly-cal','p-habits','t-hab-weekly','Review calendar','',
  NULL,0,'done',datetime('now','localtime','-1 days'),1,datetime('now','localtime','-13 days'),NULL),
-- A grandchild two levels below the periodicity, to show inheritance isn't
-- just one hop: neither this nor its parent has periodicity of its own.
 ('t-hab-weekly-plan','p-habits','t-hab-weekly','Plan next week','',
  NULL,0,'todo',NULL,2,datetime('now','localtime','-13 days'),NULL),
 ('t-hab-weekly-priorities','p-habits','t-hab-weekly-plan','Draft priorities','',
  NULL,0,'todo',NULL,1,datetime('now','localtime','-13 days'),NULL);
