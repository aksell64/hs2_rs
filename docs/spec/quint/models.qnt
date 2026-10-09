module n4f1v6 {
  import main(committee_size = 4, FAULTY = Set(3), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n4f1v20 {
  import main(committee_size = 4, FAULTY = Set(3), BYZANTINE = Set(), MAX_VIEW = 20, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n7f2v6 {
  import main(committee_size = 7, FAULTY = Set(5, 6), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}
