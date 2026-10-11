module n4f1v6 {
  import main(committee_size = 4, FAULTY = Set(3), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n4f1v20 {
  import main(committee_size = 4, FAULTY = Set(3), BYZANTINE = Set(), MAX_VIEW = 20, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n7f2v6 {
  import main(committee_size = 7, FAULTY = Set(5, 6), BYZANTINE = Set(), MAX_VIEW = 6, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n4f0v20 {
  import main(committee_size = 4, FAULTY = Set(), BYZANTINE = Set(), MAX_VIEW = 20, FAULTY_PERCENT = 0, TIMER_PERCENT = 0).* from "./main"
}

// Fewer faulty nodes than f: correct nodes outnumber a quorum, so a block that one correct node
// missed can still be certified and blocksync is exercised. With |FAULTY| = f it never triggers.
module n4f0v20t {
  import main(committee_size = 4, FAULTY = Set(), BYZANTINE = Set(), MAX_VIEW = 20, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}

module n7f1v12 {
  import main(committee_size = 7, FAULTY = Set(6), BYZANTINE = Set(), MAX_VIEW = 12, FAULTY_PERCENT = 10, TIMER_PERCENT = 20).* from "./main"
}
